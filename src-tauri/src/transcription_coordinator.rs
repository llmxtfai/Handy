use crate::actions::ACTION_MAP;
use crate::managers::audio::AudioRecordingManager;
use log::{debug, error, warn};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager};

const DEBOUNCE: Duration = Duration::from_millis(30);
const RELEASE_GRACE: Duration = Duration::from_millis(50);
const ADMISSION_IDLE: u8 = 0;
const ADMISSION_STARTING: u8 = 1;
const ADMISSION_BUSY: u8 = 2;
const ADMISSION_MUTATING: u8 = 3;
const ADMISSION_SUPPRESSED: u8 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InputAdmission {
    ReservedStart,
    ExistingLifecycle,
    BlockedByMutation,
}

fn admit_input(state: &AtomicU8, binding_id: &str, is_pressed: bool) -> InputAdmission {
    if !is_pressed || binding_descriptor(binding_id).is_none() {
        return InputAdmission::ExistingLifecycle;
    }
    match state.compare_exchange(
        ADMISSION_IDLE,
        ADMISSION_STARTING,
        Ordering::AcqRel,
        Ordering::Acquire,
    ) {
        Ok(_) => InputAdmission::ReservedStart,
        Err(ADMISSION_MUTATING) => InputAdmission::BlockedByMutation,
        Err(_) => InputAdmission::ExistingLifecycle,
    }
}

fn reserve_mutation(state: &AtomicU8) -> bool {
    state
        .compare_exchange(
            ADMISSION_IDLE,
            ADMISSION_MUTATING,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_ok()
}

fn admission_is_busy(state: &AtomicU8) -> bool {
    matches!(
        state.load(Ordering::Acquire),
        ADMISSION_STARTING | ADMISSION_BUSY
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionMode {
    Toggle,
    PushToTalk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingDescriptor {
    pub owner_binding_id: &'static str,
    pub canonical_action_id: &'static str,
    pub interaction_mode: InteractionMode,
}

pub fn binding_descriptor(binding_id: &str) -> Option<BindingDescriptor> {
    match binding_id {
        "transcribe" => Some(BindingDescriptor {
            owner_binding_id: "transcribe",
            canonical_action_id: "transcribe",
            interaction_mode: InteractionMode::Toggle,
        }),
        "push_to_talk" => Some(BindingDescriptor {
            owner_binding_id: "push_to_talk",
            canonical_action_id: "transcribe",
            interaction_mode: InteractionMode::PushToTalk,
        }),
        "transcribe_with_post_process" => Some(BindingDescriptor {
            owner_binding_id: "transcribe_with_post_process",
            canonical_action_id: "transcribe_with_post_process",
            interaction_mode: InteractionMode::Toggle,
        }),
        "push_to_talk_with_post_process" => Some(BindingDescriptor {
            owner_binding_id: "push_to_talk_with_post_process",
            canonical_action_id: "transcribe_with_post_process",
            interaction_mode: InteractionMode::PushToTalk,
        }),
        _ => None,
    }
}

pub fn is_transcribe_binding(id: &str) -> bool {
    binding_descriptor(id).is_some()
}

#[derive(Clone)]
struct StartRequest {
    owner_binding_id: &'static str,
    canonical_action_id: &'static str,
    hotkey_string: String,
    interaction_mode: InteractionMode,
}

struct StopRequest {
    owner_binding_id: &'static str,
    canonical_action_id: &'static str,
    hotkey_string: String,
}

enum Effect {
    None,
    Start(StartRequest),
    Stop(StopRequest),
}

struct RecordingContext {
    descriptor: BindingDescriptor,
    hotkey_string: String,
}

enum Stage {
    Idle,
    Recording(RecordingContext),
    Processing,
}

struct PendingRelease {
    owner_binding_id: &'static str,
    deadline: Instant,
}

struct SuppressedPtt {
    owner_binding_id: &'static str,
    release_deadline: Option<Instant>,
}

/// Pure transcription lifecycle. The public shortcut owns the recording while
/// the canonical action selects the normal or post-processing pipeline.
struct CoordinatorLifecycle {
    stage: Stage,
    last_press: Option<Instant>,
    pending_release: Option<PendingRelease>,
    suppressed_ptt: Option<SuppressedPtt>,
}

fn publish_admission(lifecycle: &CoordinatorLifecycle, state: &AtomicU8) {
    if lifecycle.suppressed_ptt.is_some() {
        state.store(ADMISSION_SUPPRESSED, Ordering::Release);
        return;
    }
    if lifecycle.is_busy() {
        state.store(ADMISSION_BUSY, Ordering::Release);
        return;
    }
    loop {
        let current = state.load(Ordering::Acquire);
        if current == ADMISSION_IDLE || current == ADMISSION_MUTATING {
            return;
        }
        if state
            .compare_exchange(current, ADMISSION_IDLE, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            return;
        }
    }
}

impl CoordinatorLifecycle {
    fn new() -> Self {
        Self {
            stage: Stage::Idle,
            last_press: None,
            pending_release: None,
            suppressed_ptt: None,
        }
    }

    fn input(
        &mut self,
        binding_id: &str,
        hotkey_string: &str,
        is_pressed: bool,
        occurred_at: Instant,
    ) -> Effect {
        let Some(descriptor) = binding_descriptor(binding_id) else {
            return Effect::None;
        };

        let expired = self.expire_due(occurred_at);
        if !matches!(expired, Effect::None) {
            return expired;
        }

        if let Some(suppressed) = &mut self.suppressed_ptt {
            if suppressed.owner_binding_id == descriptor.owner_binding_id {
                if is_pressed {
                    suppressed.release_deadline = None;
                } else if suppressed.release_deadline.is_none() {
                    suppressed.release_deadline = Some(occurred_at + RELEASE_GRACE);
                }
            }
            return Effect::None;
        }

        if self
            .pending_release
            .as_ref()
            .is_some_and(|pending| pending.owner_binding_id == descriptor.owner_binding_id)
            && is_pressed
        {
            self.pending_release = None;
            return Effect::None;
        }

        match &self.stage {
            Stage::Idle => {
                if !is_pressed || self.press_is_debounced(occurred_at, binding_id) {
                    return Effect::None;
                }
                Effect::Start(StartRequest {
                    owner_binding_id: descriptor.owner_binding_id,
                    canonical_action_id: descriptor.canonical_action_id,
                    hotkey_string: hotkey_string.to_string(),
                    interaction_mode: descriptor.interaction_mode,
                })
            }
            Stage::Recording(context) => {
                if context.descriptor.owner_binding_id != descriptor.owner_binding_id {
                    debug!(
                        "Ignoring input for '{binding_id}': '{}' owns the recording",
                        context.descriptor.owner_binding_id
                    );
                    return Effect::None;
                }

                match context.descriptor.interaction_mode {
                    InteractionMode::Toggle if is_pressed => {
                        if self.press_is_debounced(occurred_at, binding_id) {
                            Effect::None
                        } else {
                            self.stop()
                        }
                    }
                    InteractionMode::Toggle => Effect::None,
                    InteractionMode::PushToTalk if !is_pressed => {
                        if self.pending_release.is_none() {
                            self.pending_release = Some(PendingRelease {
                                owner_binding_id: descriptor.owner_binding_id,
                                deadline: occurred_at + RELEASE_GRACE,
                            });
                        }
                        Effect::None
                    }
                    InteractionMode::PushToTalk => Effect::None,
                }
            }
            Stage::Processing => {
                debug!("Ignoring input for '{binding_id}': pipeline busy");
                Effect::None
            }
        }
    }

    fn recording_started(&mut self, request: StartRequest) {
        if !matches!(self.stage, Stage::Idle) {
            return;
        }
        self.stage = Stage::Recording(RecordingContext {
            descriptor: BindingDescriptor {
                owner_binding_id: request.owner_binding_id,
                canonical_action_id: request.canonical_action_id,
                interaction_mode: request.interaction_mode,
            },
            hotkey_string: request.hotkey_string,
        });
    }

    fn next_deadline(&self) -> Option<Instant> {
        let pending = self
            .pending_release
            .as_ref()
            .map(|pending| pending.deadline);
        let suppressed = self
            .suppressed_ptt
            .as_ref()
            .and_then(|suppressed| suppressed.release_deadline);
        match (pending, suppressed) {
            (Some(left), Some(right)) => Some(left.min(right)),
            (Some(deadline), None) | (None, Some(deadline)) => Some(deadline),
            (None, None) => None,
        }
    }

    fn expire(&mut self, now: Instant) -> Effect {
        self.expire_due(now)
    }

    fn expire_due(&mut self, now: Instant) -> Effect {
        if !self
            .pending_release
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            if self
                .suppressed_ptt
                .as_ref()
                .and_then(|suppressed| suppressed.release_deadline)
                .is_some_and(|deadline| now >= deadline)
            {
                self.suppressed_ptt = None;
            }
            return Effect::None;
        }
        self.pending_release = None;
        self.stop()
    }

    fn cancel(&mut self, recording_was_active: bool) {
        self.suppressed_ptt = match &self.stage {
            Stage::Recording(context)
                if context.descriptor.interaction_mode == InteractionMode::PushToTalk =>
            {
                Some(SuppressedPtt {
                    owner_binding_id: context.descriptor.owner_binding_id,
                    release_deadline: self
                        .pending_release
                        .as_ref()
                        .map(|pending| pending.deadline),
                })
            }
            _ => None,
        };
        self.pending_release = None;
        if !matches!(self.stage, Stage::Processing)
            && (recording_was_active || matches!(self.stage, Stage::Recording(_)))
        {
            self.stage = Stage::Idle;
        }
    }

    fn processing_finished(&mut self) {
        self.pending_release = None;
        self.suppressed_ptt = None;
        self.stage = Stage::Idle;
    }

    fn is_busy(&self) -> bool {
        !matches!(self.stage, Stage::Idle)
    }

    fn stop(&mut self) -> Effect {
        let Stage::Recording(context) = std::mem::replace(&mut self.stage, Stage::Processing)
        else {
            return Effect::None;
        };
        Effect::Stop(StopRequest {
            owner_binding_id: context.descriptor.owner_binding_id,
            canonical_action_id: context.descriptor.canonical_action_id,
            hotkey_string: context.hotkey_string,
        })
    }

    fn press_is_debounced(&mut self, occurred_at: Instant, binding_id: &str) -> bool {
        if self
            .last_press
            .is_some_and(|previous| occurred_at.saturating_duration_since(previous) < DEBOUNCE)
        {
            debug!("Debounced press for '{binding_id}'");
            return true;
        }
        self.last_press = Some(occurred_at);
        false
    }
}

enum Command {
    Input {
        binding_id: String,
        hotkey_string: String,
        is_pressed: bool,
        occurred_at: Instant,
    },
    Cancel {
        recording_was_active: bool,
    },
    ProcessingFinished,
}

/// Serializes transcription lifecycle events through one thread.
pub struct TranscriptionCoordinator {
    tx: Sender<Command>,
    admission: Arc<AtomicU8>,
}

/// Holds the coordinator's idle admission slot for one complete shortcut
/// mutation. While held, a new recording cannot be admitted.
pub struct ShortcutMutationGuard {
    admission: Arc<AtomicU8>,
}

impl Drop for ShortcutMutationGuard {
    fn drop(&mut self) {
        let _ = self.admission.compare_exchange(
            ADMISSION_MUTATING,
            ADMISSION_IDLE,
            Ordering::AcqRel,
            Ordering::Acquire,
        );
    }
}

impl TranscriptionCoordinator {
    pub fn new(app: AppHandle) -> Self {
        let (tx, rx) = mpsc::channel();
        let admission = Arc::new(AtomicU8::new(ADMISSION_IDLE));
        let worker_admission = Arc::clone(&admission);

        thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut lifecycle = CoordinatorLifecycle::new();
                loop {
                    let command = if let Some(deadline) = lifecycle.next_deadline() {
                        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
                            Ok(command) => command,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let effect = lifecycle.expire(Instant::now());
                                apply_effect(&app, &mut lifecycle, &worker_admission, effect);
                                publish_admission(&lifecycle, &worker_admission);
                                continue;
                            }
                            Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        }
                    } else {
                        match rx.recv() {
                            Ok(command) => command,
                            Err(_) => break,
                        }
                    };

                    match command {
                        Command::Input {
                            binding_id,
                            hotkey_string,
                            is_pressed,
                            occurred_at,
                        } => {
                            let effect = lifecycle.input(
                                &binding_id,
                                &hotkey_string,
                                is_pressed,
                                occurred_at,
                            );
                            apply_effect(&app, &mut lifecycle, &worker_admission, effect);
                        }
                        Command::Cancel {
                            recording_was_active,
                        } => lifecycle.cancel(recording_was_active),
                        Command::ProcessingFinished => lifecycle.processing_finished(),
                    }
                    publish_admission(&lifecycle, &worker_admission);
                }
                debug!("Transcription coordinator exited");
            }));
            if let Err(error) = result {
                error!("Transcription coordinator panicked: {error:?}");
            }
            worker_admission.store(ADMISSION_IDLE, Ordering::Release);
        });

        Self { tx, admission }
    }

    pub fn send_input(&self, binding_id: &str, hotkey_string: &str, is_pressed: bool) {
        let admission = admit_input(&self.admission, binding_id, is_pressed);
        if admission == InputAdmission::BlockedByMutation {
            debug!("Ignoring input for '{binding_id}': shortcuts are being updated");
            return;
        }
        if self
            .tx
            .send(Command::Input {
                binding_id: binding_id.to_string(),
                hotkey_string: hotkey_string.to_string(),
                is_pressed,
                occurred_at: Instant::now(),
            })
            .is_err()
        {
            if admission == InputAdmission::ReservedStart {
                let _ = self.admission.compare_exchange(
                    ADMISSION_STARTING,
                    ADMISSION_IDLE,
                    Ordering::AcqRel,
                    Ordering::Acquire,
                );
            }
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn is_busy(&self) -> bool {
        admission_is_busy(&self.admission)
    }

    /// Atomically reserve Idle for a full shortcut mutation. Callers must hold
    /// the returned guard through validation, backend changes, and persistence.
    pub fn try_begin_shortcut_mutation(&self) -> Option<ShortcutMutationGuard> {
        reserve_mutation(&self.admission).then_some(ShortcutMutationGuard {
            admission: Arc::clone(&self.admission),
        })
    }

    pub fn notify_cancel(&self, recording_was_active: bool) {
        if self
            .tx
            .send(Command::Cancel {
                recording_was_active,
            })
            .is_err()
        {
            warn!("Transcription coordinator channel closed");
        }
    }

    pub fn notify_processing_finished(&self) {
        if self.tx.send(Command::ProcessingFinished).is_err() {
            warn!("Transcription coordinator channel closed");
        }
    }
}

fn apply_effect(
    app: &AppHandle,
    lifecycle: &mut CoordinatorLifecycle,
    admission: &AtomicU8,
    effect: Effect,
) {
    match effect {
        Effect::None => {}
        Effect::Start(request) => {
            if admission.load(Ordering::Acquire) != ADMISSION_STARTING
                && admission
                    .compare_exchange(
                        ADMISSION_IDLE,
                        ADMISSION_STARTING,
                        Ordering::AcqRel,
                        Ordering::Acquire,
                    )
                    .is_err()
            {
                debug!(
                    "Ignoring start for '{}': lifecycle admission is unavailable",
                    request.owner_binding_id
                );
                return;
            }
            let Some(action) = ACTION_MAP.get(request.canonical_action_id) else {
                warn!(
                    "No action in ACTION_MAP for '{}'",
                    request.canonical_action_id
                );
                return;
            };
            action.start(
                app,
                request.canonical_action_id,
                request.hotkey_string.as_str(),
            );
            if app
                .try_state::<Arc<AudioRecordingManager>>()
                .is_some_and(|audio| audio.is_recording())
            {
                lifecycle.recording_started(request);
            } else {
                debug!("Start did not begin recording; staying idle");
            }
        }
        Effect::Stop(request) => {
            let Some(action) = ACTION_MAP.get(request.canonical_action_id) else {
                warn!(
                    "No action in ACTION_MAP for '{}'",
                    request.canonical_action_id
                );
                return;
            };
            debug!("Stopping recording owned by '{}'", request.owner_binding_id);
            action.stop(
                app,
                request.canonical_action_id,
                request.hotkey_string.as_str(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(start: Instant, millis: u64) -> Instant {
        start + Duration::from_millis(millis)
    }

    fn start(lifecycle: &mut CoordinatorLifecycle, binding_id: &str, now: Instant) -> StartRequest {
        let Effect::Start(request) = lifecycle.input(binding_id, "shortcut", true, now) else {
            panic!("expected start");
        };
        lifecycle.recording_started(request.clone());
        request
    }

    #[test]
    fn public_bindings_have_exact_descriptors() {
        let cases = [
            ("transcribe", "transcribe", InteractionMode::Toggle),
            ("push_to_talk", "transcribe", InteractionMode::PushToTalk),
            (
                "transcribe_with_post_process",
                "transcribe_with_post_process",
                InteractionMode::Toggle,
            ),
            (
                "push_to_talk_with_post_process",
                "transcribe_with_post_process",
                InteractionMode::PushToTalk,
            ),
        ];
        for (owner, canonical, mode) in cases {
            assert_eq!(
                binding_descriptor(owner),
                Some(BindingDescriptor {
                    owner_binding_id: owner,
                    canonical_action_id: canonical,
                    interaction_mode: mode,
                })
            );
        }
        assert_eq!(binding_descriptor("cancel"), None);
    }

    #[test]
    fn all_recording_bindings_use_the_coordinator() {
        for id in [
            "transcribe",
            "push_to_talk",
            "transcribe_with_post_process",
            "push_to_talk_with_post_process",
        ] {
            assert!(is_transcribe_binding(id));
        }
        assert!(!is_transcribe_binding("cancel"));
    }

    #[test]
    fn toggle_starts_and_stops_on_owner_presses() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "transcribe", now);
        let Effect::Stop(request) = lifecycle.input("transcribe", "shortcut", true, at(now, 100))
        else {
            panic!("expected stop");
        };
        assert_eq!(request.owner_binding_id, "transcribe");
        assert_eq!(request.canonical_action_id, "transcribe");
        assert!(lifecycle.is_busy());
    }

    #[test]
    fn push_to_talk_stops_after_release_grace() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        assert!(matches!(
            lifecycle.input("push_to_talk", "shortcut", false, at(now, 100)),
            Effect::None
        ));
        assert_eq!(lifecycle.next_deadline(), Some(at(now, 150)));
        let Effect::Stop(request) = lifecycle.expire(at(now, 150)) else {
            panic!("expected stop");
        };
        assert_eq!(request.owner_binding_id, "push_to_talk");
        assert_eq!(request.canonical_action_id, "transcribe");
    }

    #[test]
    fn different_public_binding_cannot_stop_recording_owner() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "transcribe", now);
        assert!(matches!(
            lifecycle.input("push_to_talk", "other", true, at(now, 100)),
            Effect::None
        ));
        assert!(matches!(
            lifecycle.input("transcribe", "shortcut", true, at(now, 200)),
            Effect::Stop(_)
        ));
    }

    #[test]
    fn post_processing_owner_keeps_canonical_action_on_stop() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        let request = start(&mut lifecycle, "push_to_talk_with_post_process", now);
        assert_eq!(request.canonical_action_id, "transcribe_with_post_process");
        let _ = lifecycle.input(
            "push_to_talk_with_post_process",
            "shortcut",
            false,
            at(now, 100),
        );
        let Effect::Stop(request) = lifecycle.expire(at(now, 150)) else {
            panic!("expected stop");
        };
        assert_eq!(request.owner_binding_id, "push_to_talk_with_post_process");
        assert_eq!(request.canonical_action_id, "transcribe_with_post_process");
    }

    #[test]
    fn busy_state_tracks_recording_processing_and_completion() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        assert!(!lifecycle.is_busy());
        let Effect::Start(request) = lifecycle.input("transcribe", "shortcut", true, now) else {
            panic!("expected start");
        };
        assert!(!lifecycle.is_busy());
        lifecycle.recording_started(request);
        assert!(lifecycle.is_busy());
        let _ = lifecycle.input("transcribe", "shortcut", true, at(now, 100));
        assert!(lifecycle.is_busy());
        lifecycle.processing_finished();
        assert!(!lifecycle.is_busy());
    }

    #[test]
    fn busy_state_is_published_for_shortcut_mutation_guards() {
        let now = Instant::now();
        let admission = AtomicU8::new(ADMISSION_STARTING);
        let mut lifecycle = CoordinatorLifecycle::new();

        let Effect::Start(request) = lifecycle.input("transcribe", "shortcut", true, now) else {
            panic!("expected start");
        };
        lifecycle.recording_started(request);
        publish_admission(&lifecycle, &admission);
        assert!(admission_is_busy(&admission));

        let _ = lifecycle.input("transcribe", "shortcut", true, at(now, 100));
        publish_admission(&lifecycle, &admission);
        assert!(admission_is_busy(&admission));

        lifecycle.processing_finished();
        publish_admission(&lifecycle, &admission);
        assert!(!admission_is_busy(&admission));
    }

    #[test]
    fn cancellation_returns_recording_to_idle_and_waits_out_physical_release() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, 100));
        lifecycle.cancel(true);
        assert!(!lifecycle.is_busy());
        assert_eq!(lifecycle.next_deadline(), Some(at(now, 150)));
        assert!(matches!(lifecycle.expire(at(now, 150)), Effect::None));
        assert_eq!(lifecycle.next_deadline(), None);
    }

    #[test]
    fn processing_gate_ignores_inputs_until_finished() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "transcribe", now);
        let _ = lifecycle.input("transcribe", "shortcut", true, at(now, 100));
        assert!(matches!(
            lifecycle.input("transcribe_with_post_process", "other", true, at(now, 200)),
            Effect::None
        ));
        lifecycle.cancel(false);
        assert!(lifecycle.is_busy());
        lifecycle.processing_finished();
        assert!(matches!(
            lifecycle.input("transcribe_with_post_process", "other", true, at(now, 300)),
            Effect::Start(_)
        ));
    }

    #[test]
    fn x11_release_press_repeat_keeps_ptt_recording() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, 100));
        let _ = lifecycle.input("push_to_talk", "shortcut", true, at(now, 110));
        assert!(lifecycle.is_busy());
        assert_eq!(lifecycle.next_deadline(), None);
        assert!(matches!(lifecycle.expire(at(now, 200)), Effect::None));
    }

    #[test]
    fn ptt_repeat_press_at_49ms_cancels_pending_release() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, 100));

        assert!(matches!(
            lifecycle.input("push_to_talk", "shortcut", true, at(now, 149)),
            Effect::None
        ));
        assert_eq!(lifecycle.next_deadline(), None);
        assert!(lifecycle.is_busy());
    }

    #[test]
    fn ptt_press_at_or_after_50ms_expires_release_and_is_ignored() {
        for press_at in [150, 151] {
            let now = Instant::now();
            let mut lifecycle = CoordinatorLifecycle::new();
            start(&mut lifecycle, "push_to_talk", now);
            let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, 100));

            let Effect::Stop(request) =
                lifecycle.input("push_to_talk", "shortcut", true, at(now, press_at))
            else {
                panic!("release must expire before the new input at {press_at}ms");
            };
            assert_eq!(request.owner_binding_id, "push_to_talk");
            assert!(lifecycle.is_busy());
        }
    }

    #[test]
    fn admission_reserves_idle_before_start_and_failed_start_clears_it() {
        let state = AtomicU8::new(ADMISSION_IDLE);
        assert_eq!(
            admit_input(&state, "transcribe", true),
            InputAdmission::ReservedStart
        );
        assert!(admission_is_busy(&state));

        let lifecycle = CoordinatorLifecycle::new();
        publish_admission(&lifecycle, &state);
        assert!(!admission_is_busy(&state));
    }

    #[test]
    fn admission_does_not_reserve_irrelevant_events_or_busy_inputs() {
        let state = AtomicU8::new(ADMISSION_IDLE);
        assert_eq!(
            admit_input(&state, "push_to_talk", false),
            InputAdmission::ExistingLifecycle
        );
        assert_eq!(state.load(Ordering::Acquire), ADMISSION_IDLE);

        state.store(ADMISSION_BUSY, Ordering::Release);
        assert_eq!(
            admit_input(&state, "transcribe", true),
            InputAdmission::ExistingLifecycle
        );
        assert_eq!(state.load(Ordering::Acquire), ADMISSION_BUSY);
    }

    #[test]
    fn shortcut_mutation_reservation_blocks_concurrent_start_admission() {
        let state = AtomicU8::new(ADMISSION_IDLE);
        assert!(reserve_mutation(&state));
        assert_eq!(
            admit_input(&state, "transcribe", true),
            InputAdmission::BlockedByMutation
        );
        assert!(!admission_is_busy(&state));
    }

    #[test]
    fn owned_mutation_guard_holds_reservation_until_dropped() {
        let admission = Arc::new(AtomicU8::new(ADMISSION_IDLE));
        assert!(reserve_mutation(&admission));
        let guard = ShortcutMutationGuard {
            admission: Arc::clone(&admission),
        };
        let moved_guard = guard;
        assert_eq!(
            admit_input(&admission, "transcribe", true),
            InputAdmission::BlockedByMutation
        );
        drop(moved_guard);
        assert_eq!(admission.load(Ordering::Acquire), ADMISSION_IDLE);
    }

    #[test]
    fn cancel_while_ptt_held_suppresses_repeat_burst_until_real_release() {
        let now = Instant::now();
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        lifecycle.cancel(true);

        for offset in [100, 110, 120] {
            assert!(matches!(
                lifecycle.input("push_to_talk", "shortcut", true, at(now, offset)),
                Effect::None
            ));
            assert!(matches!(
                lifecycle.input("transcribe", "other", true, at(now, offset + 1)),
                Effect::None
            ));
            let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, offset + 2));
        }

        assert!(matches!(lifecycle.expire(at(now, 171)), Effect::None));
        assert!(matches!(
            lifecycle.input("push_to_talk", "shortcut", true, at(now, 172)),
            Effect::Start(_)
        ));
    }

    #[test]
    fn cancel_held_blocks_mutation_until_genuine_release_grace() {
        let now = Instant::now();
        let admission = AtomicU8::new(ADMISSION_BUSY);
        let mut lifecycle = CoordinatorLifecycle::new();
        start(&mut lifecycle, "push_to_talk", now);
        lifecycle.cancel(true);
        publish_admission(&lifecycle, &admission);

        assert!(!admission_is_busy(&admission));
        assert!(!reserve_mutation(&admission));
        assert!(matches!(
            lifecycle.input("push_to_talk", "shortcut", true, at(now, 100)),
            Effect::None
        ));
        let _ = lifecycle.input("push_to_talk", "shortcut", false, at(now, 110));
        publish_admission(&lifecycle, &admission);
        assert!(!reserve_mutation(&admission));

        let _ = lifecycle.expire(at(now, 160));
        publish_admission(&lifecycle, &admission);
        assert!(reserve_mutation(&admission));
    }
}
