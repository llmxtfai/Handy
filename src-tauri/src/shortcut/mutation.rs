use std::collections::HashMap;
use std::sync::Mutex;

use log::{error, warn};
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Manager};

use crate::settings::{self, KeyboardImplementation, ShortcutBinding};
use crate::transcription_coordinator::ShortcutMutationGuard;
use crate::TranscriptionCoordinator;

use super::{
    register_shortcut_for_implementation, unregister_shortcut_for_implementation,
    validate_shortcut_for_implementation,
};

pub(super) const OPTIONAL_BINDING_IDS: [&str; 4] = [
    "transcribe",
    "push_to_talk",
    "transcribe_with_post_process",
    "push_to_talk_with_post_process",
];
const NORMAL_BINDING_IDS: [&str; 2] = ["transcribe", "push_to_talk"];
pub(super) const POST_PROCESS_BINDING_IDS: [&str; 2] = [
    "transcribe_with_post_process",
    "push_to_talk_with_post_process",
];
pub(super) const RECORDING_IN_PROGRESS_ERROR: &str = "recording_in_progress";
const FINAL_NORMAL_BINDING_ERROR: &str = "final_normal_binding_required";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaptureKind {
    Global,
    HandyKeys,
}

pub(super) struct ActiveCapture {
    pub(super) session: CaptureSession,
    pub(super) _guard: ShortcutMutationGuard,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct CaptureSession {
    pub(super) binding_id: String,
    pub(super) kind: CaptureKind,
    pub(super) binding_registered: bool,
}

impl CaptureSession {
    pub(super) fn new(binding_id: &str, kind: CaptureKind) -> Self {
        Self {
            binding_id: binding_id.to_string(),
            kind,
            binding_registered: false,
        }
    }

    pub(super) fn commit(&mut self, binding_registered: bool) -> bool {
        self.binding_registered = binding_registered;
        self.kind == CaptureKind::Global
    }
}

#[derive(Default)]
pub struct ShortcutCaptureState(pub(super) Mutex<Option<ActiveCapture>>);

pub(super) fn is_optional_binding(id: &str) -> bool {
    OPTIONAL_BINDING_IDS.contains(&id)
}

pub(super) fn is_capturable_binding(id: &str) -> bool {
    is_optional_binding(id) || id == "cancel"
}

pub(super) fn is_assigned(binding: &ShortcutBinding) -> bool {
    !binding.current_binding.trim().is_empty()
}

pub(super) fn should_be_registered(id: &str, post_process_enabled: bool, binding: &str) -> bool {
    !binding.trim().is_empty() && (!POST_PROCESS_BINDING_IDS.contains(&id) || post_process_enabled)
}

pub(super) fn canonical_binding(id: &str, binding: &str) -> String {
    if is_optional_binding(id) && binding.trim().is_empty() {
        String::new()
    } else {
        binding.to_string()
    }
}

pub(super) fn validate_optional_assignment(
    bindings: &HashMap<String, ShortcutBinding>,
    id: &str,
    new_binding: &str,
) -> Result<(), String> {
    if !is_optional_binding(id) || !new_binding.trim().is_empty() {
        return Ok(());
    }
    if NORMAL_BINDING_IDS.contains(&id)
        && NORMAL_BINDING_IDS.iter().all(|normal_id| {
            *normal_id == id
                || bindings
                    .get(*normal_id)
                    .is_none_or(|binding| !is_assigned(binding))
        })
    {
        return Err(FINAL_NORMAL_BINDING_ERROR.to_string());
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum BindingMutation {
    Clear,
    Assign(String),
}

pub(super) fn reset_action(binding: &ShortcutBinding) -> BindingMutation {
    if binding.default_binding.trim().is_empty() && is_optional_binding(&binding.id) {
        BindingMutation::Clear
    } else {
        BindingMutation::Assign(binding.default_binding.clone())
    }
}

pub(super) trait BindingBackend {
    fn validate(&mut self, binding: &str) -> Result<(), String>;
    fn unregister(&mut self, binding: &ShortcutBinding) -> Result<(), String>;
    fn register(&mut self, binding: &ShortcutBinding) -> Result<(), String>;
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct TransactionFailure {
    pub(super) code: &'static str,
    pub(super) details: String,
}

pub(super) fn validate_non_empty_binding(
    backend: &mut impl BindingBackend,
    binding: &ShortcutBinding,
) -> Result<(), TransactionFailure> {
    if is_assigned(binding) {
        backend
            .validate(&binding.current_binding)
            .map_err(|details| TransactionFailure {
                code: "shortcut_validation_failed",
                details,
            })?;
    }
    Ok(())
}

pub(super) fn restore_capture_binding(
    backend: &mut impl BindingBackend,
    binding: &ShortcutBinding,
) -> Result<(), String> {
    backend
        .register(binding)
        .map_err(|error| format!("shortcut_capture_restoration_failed: {error}"))
}

pub(super) fn suspend_capture_binding(
    backend: &mut impl BindingBackend,
    binding: &ShortcutBinding,
) -> Result<(), String> {
    backend
        .unregister(binding)
        .map_err(|error| format!("shortcut_capture_suspend_failed: {error}"))
}

pub(super) fn replace_registered_binding(
    backend: &mut impl BindingBackend,
    old: &ShortcutBinding,
    new: &ShortcutBinding,
    old_is_suspended: bool,
) -> Result<(), TransactionFailure> {
    validate_non_empty_binding(backend, new)?;
    if !old_is_suspended && is_assigned(old) {
        backend
            .unregister(old)
            .map_err(|details| TransactionFailure {
                code: "shortcut_unregistration_failed",
                details,
            })?;
    }
    if is_assigned(new) {
        if let Err(register_error) = backend.register(new) {
            if is_assigned(old) {
                if let Err(restore_error) = backend.register(old) {
                    return Err(TransactionFailure {
                        code: "shortcut_restoration_failed",
                        details: format!(
                            "registration failed: {register_error}; restoration failed: {restore_error}"
                        ),
                    });
                }
            }
            return Err(TransactionFailure {
                code: "shortcut_registration_failed",
                details: register_error,
            });
        }
    }
    Ok(())
}

pub(super) fn update_registration_set(
    backend: &mut impl BindingBackend,
    bindings: &[ShortcutBinding],
    enable: bool,
) -> Result<(), TransactionFailure> {
    let mut completed: Vec<&ShortcutBinding> = Vec::new();
    for binding in bindings {
        let result = if enable {
            backend.register(binding)
        } else {
            backend.unregister(binding)
        };
        if let Err(primary) = result {
            let mut restoration_errors = Vec::new();
            for previous in completed.into_iter().rev() {
                let rollback = if enable {
                    backend.unregister(previous)
                } else {
                    backend.register(previous)
                };
                if let Err(error) = rollback {
                    restoration_errors.push(error);
                }
            }
            if restoration_errors.is_empty() {
                return Err(TransactionFailure {
                    code: "shortcut_set_update_failed",
                    details: primary,
                });
            }
            return Err(TransactionFailure {
                code: "shortcut_set_restoration_failed",
                details: format!("{primary}; restoration: {}", restoration_errors.join("; ")),
            });
        }
        completed.push(binding);
    }
    Ok(())
}

pub(super) fn registration_cleanup_error(primary: &str, cleanup_errors: &[String]) -> String {
    if cleanup_errors.is_empty() {
        format!("handy_keys_init_registration_failed: {primary}")
    } else {
        format!(
            "handy_keys_init_cleanup_failed: registration={primary}; cleanup={}",
            cleanup_errors.join("; ")
        )
    }
}

pub(super) fn preflight_assigned_bindings(
    bindings: &HashMap<String, ShortcutBinding>,
    post_process_enabled: bool,
    mut validate: impl FnMut(&str) -> Result<(), String>,
) -> Result<(), String> {
    for (id, binding) in bindings {
        if id == "cancel"
            || !should_be_registered(id, post_process_enabled, &binding.current_binding)
        {
            continue;
        }
        validate(&binding.current_binding)?;
    }
    Ok(())
}

pub(super) struct AppBindingBackend<'a> {
    pub(super) app: &'a AppHandle,
    pub(super) implementation: KeyboardImplementation,
}

impl BindingBackend for AppBindingBackend<'_> {
    fn validate(&mut self, binding: &str) -> Result<(), String> {
        validate_shortcut_for_implementation(binding, self.implementation)
    }

    fn unregister(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
        unregister_shortcut_for_implementation(self.app, binding.clone(), self.implementation)
    }

    fn register(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
        register_shortcut_for_implementation(self.app, binding.clone(), self.implementation)
    }
}

pub(super) fn with_shortcut_mutation<T>(
    app: &AppHandle,
    operation: impl FnOnce() -> Result<T, String>,
) -> Result<T, String> {
    let coordinator = app
        .try_state::<TranscriptionCoordinator>()
        .ok_or_else(|| "transcription_coordinator_unavailable".to_string())?;
    let _guard = coordinator
        .try_begin_shortcut_mutation()
        .ok_or_else(|| RECORDING_IN_PROGRESS_ERROR.to_string())?;
    operation()
}

pub(super) fn begin_shortcut_capture(
    app: &AppHandle,
    binding_id: &str,
    kind: CaptureKind,
    start_capture: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    if !is_capturable_binding(binding_id) {
        return Err("binding_not_capturable".to_string());
    }
    let coordinator = app
        .try_state::<TranscriptionCoordinator>()
        .ok_or_else(|| "transcription_coordinator_unavailable".to_string())?;
    let guard = coordinator
        .try_begin_shortcut_mutation()
        .ok_or_else(|| RECORDING_IN_PROGRESS_ERROR.to_string())?;
    let capture_state = app
        .try_state::<ShortcutCaptureState>()
        .ok_or_else(|| "shortcut_capture_state_unavailable".to_string())?;
    let mut active = capture_state
        .0
        .lock()
        .map_err(|_| "shortcut_capture_state_poisoned".to_string())?;
    if active.is_some() {
        return Err(RECORDING_IN_PROGRESS_ERROR.to_string());
    }

    let settings = settings::get_settings(app);
    let binding = settings.bindings.get(binding_id).cloned();
    if let Some(binding) = &binding {
        if should_be_registered(
            binding_id,
            settings.post_process_enabled,
            &binding.current_binding,
        ) {
            let mut backend = AppBindingBackend {
                app,
                implementation: settings.keyboard_implementation,
            };
            suspend_capture_binding(&mut backend, binding)?;
        }
    }

    if let Err(error) = start_capture() {
        if let Some(binding) = binding {
            if should_be_registered(
                binding_id,
                settings.post_process_enabled,
                &binding.current_binding,
            ) {
                register_shortcut_for_implementation(
                    app,
                    binding,
                    settings.keyboard_implementation,
                )
                .map_err(|restore| {
                    format!("shortcut_capture_restoration_failed: {error}; {restore}")
                })?;
            }
        }
        return Err(error);
    }

    *active = Some(ActiveCapture {
        session: CaptureSession::new(binding_id, kind),
        _guard: guard,
    });
    Ok(())
}

pub(super) fn finish_shortcut_capture(
    app: &AppHandle,
    kind: CaptureKind,
    stop_capture: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    let capture_state = app
        .try_state::<ShortcutCaptureState>()
        .ok_or_else(|| "shortcut_capture_state_unavailable".to_string())?;
    let mut active = capture_state
        .0
        .lock()
        .map_err(|_| "shortcut_capture_state_poisoned".to_string())?;
    let Some(capture) = active.as_mut() else {
        drop(active);
        return with_shortcut_mutation(app, stop_capture);
    };
    if capture.session.kind != kind {
        return Err(RECORDING_IN_PROGRESS_ERROR.to_string());
    }

    stop_capture()?;
    if !capture.session.binding_registered {
        let settings = settings::get_settings(app);
        if let Some(binding) = settings.bindings.get(&capture.session.binding_id).cloned() {
            if should_be_registered(
                &capture.session.binding_id,
                settings.post_process_enabled,
                &binding.current_binding,
            ) {
                register_shortcut_for_implementation(
                    app,
                    binding,
                    settings.keyboard_implementation,
                )
                .map_err(|error| format!("shortcut_capture_restoration_failed: {error}"))?;
            }
        }
    }
    active.take();
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn abort_shortcut_capture(app: AppHandle, id: String) -> Result<(), String> {
    if !is_capturable_binding(&id) {
        return Err("binding_not_capturable".to_string());
    }
    let capture_state = app
        .try_state::<ShortcutCaptureState>()
        .ok_or_else(|| "shortcut_capture_state_unavailable".to_string())?;
    let mut active = capture_state
        .0
        .lock()
        .map_err(|_| "shortcut_capture_state_poisoned".to_string())?;
    let Some(capture) = active.as_mut() else {
        return Ok(());
    };
    if capture.session.binding_id != id || capture.session.kind != CaptureKind::Global {
        return Err("shortcut_capture_mismatch".to_string());
    }

    if !capture.session.binding_registered {
        let settings = settings::get_settings(&app);
        if let Some(binding) = settings.bindings.get(&id).cloned() {
            if should_be_registered(&id, settings.post_process_enabled, &binding.current_binding) {
                let mut backend = AppBindingBackend {
                    app: &app,
                    implementation: settings.keyboard_implementation,
                };
                restore_capture_binding(&mut backend, &binding)?;
            }
        }
    }
    active.take();
    Ok(())
}

#[derive(Serialize, Type)]
pub struct BindingResponse {
    success: bool,
    binding: Option<ShortcutBinding>,
    error: Option<String>,
}

#[tauri::command]
#[specta::specta]
pub fn change_binding(
    app: AppHandle,
    id: String,
    binding: String,
) -> Result<BindingResponse, String> {
    if let Some(capture_state) = app.try_state::<ShortcutCaptureState>() {
        let mut active = capture_state
            .0
            .lock()
            .map_err(|_| "shortcut_capture_state_poisoned".to_string())?;
        if let Some(capture) = active.as_mut() {
            if capture.session.binding_id != id {
                return Err(RECORDING_IN_PROGRESS_ERROR.to_string());
            }
            let settings = settings::get_settings(&app);
            let response = change_binding_with_settings(
                &app,
                &id,
                &binding,
                settings.clone(),
                !capture.session.binding_registered,
            )?;
            if response.success {
                let binding_registered = response.binding.as_ref().is_some_and(|binding| {
                    should_be_registered(
                        &id,
                        settings.post_process_enabled,
                        &binding.current_binding,
                    )
                });
                if capture.session.commit(binding_registered) {
                    active.take();
                }
            } else if response
                .error
                .as_deref()
                .is_some_and(|error| error.starts_with("shortcut_registration_failed:"))
            {
                capture.session.binding_registered = true;
            }
            return Ok(response);
        }
    }
    with_shortcut_mutation(&app, || change_binding_transaction(&app, &id, &binding))
}

fn change_binding_transaction(
    app: &AppHandle,
    id: &str,
    binding: &str,
) -> Result<BindingResponse, String> {
    let settings = settings::get_settings(app);
    change_binding_with_settings(app, id, binding, settings, false)
}

fn change_binding_with_settings(
    app: &AppHandle,
    id: &str,
    binding: &str,
    mut settings: settings::AppSettings,
    old_is_suspended: bool,
) -> Result<BindingResponse, String> {
    let binding = canonical_binding(id, binding);
    if binding.trim().is_empty() && !is_optional_binding(id) {
        return Err("Binding cannot be empty".to_string());
    }

    let binding_to_modify = match settings.bindings.get(id) {
        Some(binding) => binding.clone(),
        None => {
            let default_settings = settings::get_default_settings();
            match default_settings.bindings.get(id) {
                Some(default_binding) => {
                    warn!("Binding '{id}' not found in settings, creating from defaults");
                    default_binding.clone()
                }
                None => {
                    let error_msg = format!("Binding with id '{id}' not found in defaults");
                    warn!("change_binding error: {error_msg}");
                    return Ok(BindingResponse {
                        success: false,
                        binding: None,
                        error: Some(error_msg),
                    });
                }
            }
        }
    };

    validate_optional_assignment(&settings.bindings, id, &binding)?;
    if binding_to_modify.current_binding == binding && !old_is_suspended {
        return Ok(BindingResponse {
            success: true,
            binding: Some(binding_to_modify),
            error: None,
        });
    }

    if id == "cancel" {
        if let Some(mut updated) = settings.bindings.get(id).cloned() {
            updated.current_binding = binding;
            settings.bindings.insert(id.to_string(), updated.clone());
            settings::write_settings(app, settings);
            return Ok(BindingResponse {
                success: true,
                binding: Some(updated),
                error: None,
            });
        }
    }

    let mut updated_binding = binding_to_modify.clone();
    updated_binding.current_binding = binding;
    let backend_enabled = should_be_registered(
        id,
        settings.post_process_enabled,
        &binding_to_modify.current_binding,
    ) || should_be_registered(
        id,
        settings.post_process_enabled,
        &updated_binding.current_binding,
    );
    let mut backend = AppBindingBackend {
        app,
        implementation: settings.keyboard_implementation,
    };
    let backend_result = if backend_enabled {
        replace_registered_binding(
            &mut backend,
            &binding_to_modify,
            &updated_binding,
            old_is_suspended,
        )
    } else {
        validate_non_empty_binding(&mut backend, &updated_binding)
    };
    if let Err(failure) = backend_result {
        let error_msg = format!("{}: {}", failure.code, failure.details);
        error!("change_binding error: {error_msg}");
        return Ok(BindingResponse {
            success: false,
            binding: Some(binding_to_modify),
            error: Some(error_msg),
        });
    }

    settings
        .bindings
        .insert(id.to_string(), updated_binding.clone());
    settings::write_settings(app, settings);
    Ok(BindingResponse {
        success: true,
        binding: Some(updated_binding),
        error: None,
    })
}

#[tauri::command]
#[specta::specta]
pub fn clear_binding(app: AppHandle, id: String) -> Result<BindingResponse, String> {
    if !is_optional_binding(&id) {
        return Err("binding_cannot_be_cleared".to_string());
    }
    change_binding(app, id, String::new())
}

#[tauri::command]
#[specta::specta]
pub fn reset_binding(app: AppHandle, id: String) -> Result<BindingResponse, String> {
    with_shortcut_mutation(&app, || {
        let current_settings = settings::get_settings(&app);
        let binding = current_settings
            .bindings
            .get(&id)
            .cloned()
            .ok_or_else(|| format!("Binding with id '{id}' not found"))?;
        match reset_action(&binding) {
            BindingMutation::Clear => {
                change_binding_with_settings(&app, &id, "", current_settings, false)
            }
            BindingMutation::Assign(default_binding) => {
                change_binding_with_settings(&app, &id, &default_binding, current_settings, false)
            }
        }
    })
}

#[tauri::command]
#[specta::specta]
pub fn suspend_binding(app: AppHandle, id: String) -> Result<(), String> {
    begin_shortcut_capture(&app, &id, CaptureKind::Global, || Ok(()))
}

#[tauri::command]
#[specta::specta]
pub fn resume_binding(app: AppHandle, id: String) -> Result<(), String> {
    let capture_state = app
        .try_state::<ShortcutCaptureState>()
        .ok_or_else(|| "shortcut_capture_state_unavailable".to_string())?;
    {
        let mut active = capture_state
            .0
            .lock()
            .map_err(|_| "shortcut_capture_state_poisoned".to_string())?;
        if let Some(capture) = active.as_mut() {
            if capture.session.binding_id != id || capture.session.kind != CaptureKind::Global {
                return Err(RECORDING_IN_PROGRESS_ERROR.to_string());
            }
            if !capture.session.binding_registered {
                let settings = settings::get_settings(&app);
                if let Some(binding) = settings.bindings.get(&id).cloned() {
                    if should_be_registered(
                        &id,
                        settings.post_process_enabled,
                        &binding.current_binding,
                    ) {
                        register_shortcut_for_implementation(
                            &app,
                            binding,
                            settings.keyboard_implementation,
                        )?;
                    }
                }
            }
            active.take();
            return Ok(());
        }
    }
    with_shortcut_mutation(&app, || {
        let settings = settings::get_settings(&app);
        if let Some(binding) = settings.bindings.get(&id).cloned() {
            if !should_be_registered(&id, settings.post_process_enabled, &binding.current_binding) {
                return Ok(());
            }
            register_shortcut_for_implementation(&app, binding, settings.keyboard_implementation)
                .map_err(|error| {
                error!("resume_binding error for id '{id}': {error}");
                error
            })?;
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(id: &str, current: &str, default: &str) -> ShortcutBinding {
        ShortcutBinding {
            id: id.to_string(),
            name: id.to_string(),
            description: String::new(),
            default_binding: default.to_string(),
            current_binding: current.to_string(),
        }
    }

    fn recording_bindings(toggle: &str, ptt: &str) -> HashMap<String, ShortcutBinding> {
        [
            ("transcribe".into(), binding("transcribe", toggle, "")),
            (
                "push_to_talk".into(),
                binding("push_to_talk", ptt, "ctrl+space"),
            ),
            (
                "transcribe_with_post_process".into(),
                binding("transcribe_with_post_process", "", ""),
            ),
            (
                "push_to_talk_with_post_process".into(),
                binding("push_to_talk_with_post_process", "", "ctrl+shift+space"),
            ),
        ]
        .into_iter()
        .collect()
    }

    #[derive(Default)]
    struct FakeBackend {
        calls: Vec<String>,
        fail_validate: bool,
        fail_register: Option<String>,
        fail_unregister: Option<String>,
        fail_restore: bool,
    }

    impl BindingBackend for FakeBackend {
        fn validate(&mut self, _: &str) -> Result<(), String> {
            self.calls.push("validate".into());
            if self.fail_validate {
                Err("invalid".into())
            } else {
                Ok(())
            }
        }
        fn unregister(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
            self.calls.push(format!("unregister:{}", binding.id));
            if self.fail_unregister.as_deref() == Some(&binding.current_binding) {
                Err("unregister failed".into())
            } else {
                Ok(())
            }
        }
        fn register(&mut self, binding: &ShortcutBinding) -> Result<(), String> {
            self.calls.push(format!("register:{}", binding.id));
            if self.fail_restore && binding.current_binding == "ctrl+space" {
                return Err("restore failed".into());
            }
            if self.fail_register.as_deref() == Some(&binding.current_binding) {
                Err("duplicate".into())
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn optional_policy_and_pair_invariant() {
        assert!(OPTIONAL_BINDING_IDS.into_iter().all(is_optional_binding));
        assert!(!is_optional_binding("cancel"));
        let bindings = recording_bindings("", "ctrl+space");
        assert_eq!(
            validate_optional_assignment(&bindings, "push_to_talk", ""),
            Err(FINAL_NORMAL_BINDING_ERROR.into())
        );
        assert!(
            validate_optional_assignment(&bindings, "transcribe_with_post_process", "").is_ok()
        );
    }

    #[test]
    fn reset_blank_and_canonicalization_policy() {
        let bindings = recording_bindings("alt+space", "ctrl+space");
        assert_eq!(
            reset_action(&bindings["transcribe"]),
            BindingMutation::Clear
        );
        assert_eq!(canonical_binding("transcribe", " \t "), "");
        assert_eq!(canonical_binding("cancel", " escape "), " escape ");
    }

    #[test]
    fn backend_preflight_rejects_invalid_and_skips_disabled_post_process() {
        let mut bindings = recording_bindings("ctrl+space", "");
        bindings
            .get_mut("transcribe_with_post_process")
            .unwrap()
            .current_binding = "invalid-post".into();
        let mut checked = Vec::new();
        preflight_assigned_bindings(&bindings, false, |value| {
            checked.push(value.to_string());
            Ok(())
        })
        .unwrap();
        assert_eq!(checked, vec!["ctrl+space"]);
        assert_eq!(
            preflight_assigned_bindings(&bindings, true, |value| if value == "invalid-post" {
                Err("unsupported".into())
            } else {
                Ok(())
            }),
            Err("unsupported".into())
        );
    }

    #[test]
    fn disabled_post_process_is_not_registered_but_is_validated() {
        assert!(!should_be_registered(
            "transcribe_with_post_process",
            false,
            "ctrl+1"
        ));
        let candidate = binding("transcribe_with_post_process", "invalid", "");
        let mut backend = FakeBackend {
            fail_validate: true,
            ..Default::default()
        };
        assert_eq!(
            validate_non_empty_binding(&mut backend, &candidate)
                .unwrap_err()
                .code,
            "shortcut_validation_failed"
        );
        assert_eq!(backend.calls, vec!["validate"]);
    }

    #[test]
    fn duplicate_registration_restores_old_without_unregistering_candidate() {
        let old = binding("transcribe", "ctrl+space", "");
        let mut new = old.clone();
        new.current_binding = "ctrl+shift+space".into();
        let mut backend = FakeBackend {
            fail_register: Some(new.current_binding.clone()),
            ..Default::default()
        };
        assert_eq!(
            replace_registered_binding(&mut backend, &old, &new, false)
                .unwrap_err()
                .code,
            "shortcut_registration_failed"
        );
        assert_eq!(
            backend.calls,
            vec![
                "validate",
                "unregister:transcribe",
                "register:transcribe",
                "register:transcribe"
            ]
        );
    }

    #[test]
    fn registration_restoration_failure_is_reported() {
        let old = binding("transcribe", "ctrl+space", "");
        let mut new = old.clone();
        new.current_binding = "ctrl+shift+space".into();
        let mut backend = FakeBackend {
            fail_register: Some(new.current_binding.clone()),
            fail_restore: true,
            ..Default::default()
        };
        assert_eq!(
            replace_registered_binding(&mut backend, &old, &new, false)
                .unwrap_err()
                .code,
            "shortcut_restoration_failed"
        );
    }

    #[test]
    fn registration_set_rolls_back_both_directions_and_surfaces_failure() {
        let bindings = vec![
            binding("first", "ctrl+1", ""),
            binding("second", "ctrl+2", ""),
        ];
        let mut enable = FakeBackend {
            fail_register: Some("ctrl+2".into()),
            ..Default::default()
        };
        assert_eq!(
            update_registration_set(&mut enable, &bindings, true)
                .unwrap_err()
                .code,
            "shortcut_set_update_failed"
        );
        let mut disable = FakeBackend {
            fail_unregister: Some("ctrl+2".into()),
            ..Default::default()
        };
        assert_eq!(
            update_registration_set(&mut disable, &bindings, false)
                .unwrap_err()
                .code,
            "shortcut_set_update_failed"
        );
        assert_eq!(
            disable.calls,
            vec!["unregister:first", "unregister:second", "register:first"]
        );
        let mut cleanup = FakeBackend {
            fail_register: Some("ctrl+2".into()),
            fail_unregister: Some("ctrl+1".into()),
            ..Default::default()
        };
        assert_eq!(
            update_registration_set(&mut cleanup, &bindings, true)
                .unwrap_err()
                .code,
            "shortcut_set_restoration_failed"
        );
    }

    #[test]
    fn capture_policy_holds_handy_until_stop_and_releases_global_on_commit() {
        let mut global = CaptureSession::new("transcribe", CaptureKind::Global);
        assert!(global.commit(true));
        let mut handy = CaptureSession::new("push_to_talk", CaptureKind::HandyKeys);
        assert!(!handy.commit(true));
        assert!(OPTIONAL_BINDING_IDS
            .into_iter()
            .chain(["cancel"])
            .all(is_capturable_binding));
        assert!(!is_capturable_binding("unknown"));
    }

    #[test]
    fn capture_restoration_and_init_cleanup_errors_are_stable() {
        let mut backend = FakeBackend {
            fail_restore: true,
            ..Default::default()
        };
        let captured = binding("transcribe", "ctrl+space", "");
        assert_eq!(
            restore_capture_binding(&mut backend, &captured),
            Err("shortcut_capture_restoration_failed: restore failed".into())
        );
        backend.fail_restore = false;
        restore_capture_binding(&mut backend, &captured).unwrap();
        assert_eq!(
            backend.calls,
            vec!["register:transcribe", "register:transcribe"]
        );
        assert_eq!(
            registration_cleanup_error("duplicate", &["cleanup".into()]),
            "handy_keys_init_cleanup_failed: registration=duplicate; cleanup=cleanup"
        );
    }

    #[test]
    fn capture_suspend_failure_does_not_reregister_a_binding_that_may_still_be_live() {
        let captured = binding("transcribe", "ctrl+space", "");
        let mut backend = FakeBackend {
            fail_unregister: Some(captured.current_binding.clone()),
            ..Default::default()
        };

        assert_eq!(
            suspend_capture_binding(&mut backend, &captured),
            Err("shortcut_capture_suspend_failed: unregister failed".into())
        );
        assert_eq!(backend.calls, vec!["unregister:transcribe"]);
    }
}
