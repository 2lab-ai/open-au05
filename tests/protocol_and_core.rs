use open_au05::config::Config;
use open_au05::dictation::stt::parse_stt_response;
use open_au05::hooks::{self, AgentTracker, AGENTS};
use open_au05::input::scheduler::KeyScheduler;
use open_au05::input::{fixed_function_to_linux_key, hid_to_linux_key, MockInputEmitter};
use open_au05::protocol::{
    Control, FirmwareError, FirmwareImage, FirmwareTarget, FixedFunction, FrameCodec, Hotkey,
    LedField, LedWorkType, LightMode, ShortcutKey, Tea, VibeMessage, AUTH_PRIVATE_KEYS,
};
use open_au05::session::Session;
use open_au05::transport::is_au05_vendor_descriptor;
use open_au05::transport::mock::MockTransport;
use open_au05::transport::Transport;
use std::time::Duration;

#[test]
fn tea_roundtrip_and_zero_block_vector() {
    let zero_cipher = Tea::encrypt_block([0u8; 8]);
    // Verified against live AU05 firmware 4.4.0 trailing encrypted zero block:
    // `38 90 c4 99 a3 60 aa ..`
    assert_eq!(
        &zero_cipher[..7],
        &[0x38, 0x90, 0xC4, 0x99, 0xA3, 0x60, 0xAA]
    );
    assert_eq!(Tea::decrypt_block(zero_cipher), [0u8; 8]);

    let sample = [0x06, 0x02, 0x05, 0x01, 0x0E, 0x9E, 0x4C, 0x7A];
    let enc = Tea::encrypt_block(sample);
    assert_eq!(Tea::decrypt_block(enc), sample);
}

#[test]
fn frame_codec_roundtrip_56_bytes() {
    let msg = [
        0x01, 0x06, 0x50, 0x04, 0x01, 0x01, 0x02, 0x03, 0x08, 0x02, 0x28,
    ];
    let report = FrameCodec::encode_output_report(&msg);
    assert_eq!(report.len(), 64);
    assert_eq!(report[0], 0x55);

    let decoded = FrameCodec::decode_input_report(&report).unwrap();
    assert_eq!(decoded.bytes.len(), 56);
    assert_eq!(&decoded.bytes[..msg.len()], &msg);
}

#[test]
fn auth_reply_verified_against_captured_vector() {
    // Captured example from real AU05:
    // code 0x3F7A4C9E -> reply `06 02 05 11 01 f3 f3 42 fe`
    let frame =
        open_au05::protocol::Frame::new(vec![0x06, 0x02, 0x05, 0x11, 0x01, 0xF3, 0xF3, 0x42, 0xFE]);
    match VibeMessage::parse(&frame) {
        VibeMessage::AuthReply { key_index, code } => {
            assert_eq!(key_index, 1);
            assert_eq!(code, 0x3F7A_4C9E);
            assert_eq!(0xFE42_F3F3 ^ AUTH_PRIVATE_KEYS[1], 0x3F7A_4C9E);
        }
        other => panic!("unexpected parsed message: {other:?}"),
    }
}

#[test]
fn vendor_hid_descriptor_detection() {
    // Exact AU05 interface 3 report descriptor from live hardware
    let au05_desc: &[u8] = &[
        0x06, 0xFC, 0xFF, 0x09, 0x01, 0xA1, 0x01, 0x09, 0x02, 0x85, 0x55, 0x15, 0x00, 0x26, 0xFF,
        0x00, 0x75, 0x08, 0x95, 0x3F, 0x81, 0x02, 0x09, 0x03, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x75,
        0x08, 0x95, 0x3F, 0x91, 0x02, 0xC0,
    ];
    assert!(is_au05_vendor_descriptor(au05_desc));

    // Standard keyboard descriptor must NOT match
    let kbd_desc: &[u8] = &[0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0xC0];
    assert!(!is_au05_vendor_descriptor(kbd_desc));
}

#[test]
fn hotkey_parsing_and_rendering() {
    let keys = Hotkey::parse("super+return").unwrap();
    assert_eq!(
        keys,
        vec![
            ShortcutKey::new(3, 0x08, false),
            ShortcutKey::new(2, 0x28, false)
        ]
    );
    assert_eq!(Hotkey::render(&keys), "super+return");

    let wheel = Hotkey::parse("wheel-up").unwrap();
    assert_eq!(wheel, vec![ShortcutKey::new(7, 0x05, false)]);
    assert_eq!(Hotkey::render(&wheel), "wheel-up");

    let fn_key = Hotkey::parse("fn").unwrap();
    assert_eq!(fn_key, vec![ShortcutKey::new(0x13, 2, false)]);

    assert!(Hotkey::parse("ctrl+shift+alt+super+a").is_err());
    assert_eq!(FixedFunction::code_for_name("play-pause"), Some(7));
}

#[test]
fn mock_session_end_to_end() {
    let mut mock = MockTransport::default();
    mock.connect().unwrap();
    mock.push_key_event(Control::Talk, true);
    let mut session = Session::new(Box::new(mock), false);

    session.authenticate(Duration::from_secs(2)).unwrap();
    assert!(session.is_authenticated());

    let info = session.read_info().unwrap();
    assert_eq!(info.dongle_serial.as_deref(), Some("AU05MOCK1234"));
    assert_eq!(info.device_serial.as_deref(), Some("MIC0019999"));
    assert_eq!(info.device_uuid.as_deref(), Some("UUID-001"));
    assert!(info.mic_linked);
    assert_eq!(info.battery.as_ref().unwrap().percent, 87);

    // Settings read/write roundtrip
    session.set_sleep_time(7200).unwrap();
    session.set_noise_reduction(0, 1600, 1000).unwrap();
    session.set_light_mode(LightMode::On).unwrap();
    session.set_all_on_brightness(15).unwrap();
    session
        .set_led_field(1, LedField::WorkType, LedWorkType::Breathe as u8)
        .unwrap();

    let settings = session.read_settings().unwrap();
    assert_eq!(settings.sleep_time_secs, Some(7200));
    assert_eq!(settings.noise_reduction, Some((0, 1600, 1000)));
    let lights = settings.indicator_lights.unwrap();
    assert_eq!(lights.mode, 1);
    assert_eq!(lights.all_on_brightness, 15);
    assert_eq!(lights.leds[1].work_type, 2);

    // Button binding roundtrip
    let chord = Hotkey::parse("ctrl+shift+v").unwrap();
    session
        .set_button_shortcut(Control::Confirm, &chord)
        .unwrap();
    let buttons = session.read_buttons().unwrap();
    let confirm = buttons
        .iter()
        .find(|b| b.control == Control::Confirm)
        .unwrap();
    assert_eq!(confirm.shortcut, chord);
    assert_eq!(confirm.shortcut_text, "ctrl+shift+v");

    // Reset buttons
    session.reset_buttons().unwrap();
    let buttons_after = session.read_buttons().unwrap();
    let confirm_after = buttons_after
        .iter()
        .find(|b| b.control == Control::Confirm)
        .unwrap();
    assert!(confirm_after.shortcut.is_empty());
    assert_eq!(confirm_after.fixed_function, 0x70);
}

#[test]
fn linux_keycode_and_scheduler_refcounting() {
    assert_eq!(hid_to_linux_key(0x04), Some(30)); // A -> KEY_A
    assert_eq!(hid_to_linux_key(0x28), Some(28)); // Return -> KEY_ENTER
    assert_eq!(hid_to_linux_key(0xE0), Some(29)); // Left Ctrl -> KEY_LEFTCTRL
    assert_eq!(fixed_function_to_linux_key(7), Some(164)); // Play/Pause

    let mut emitter = MockInputEmitter::default();
    let mut sched = KeyScheduler::default();

    // Two controls share Ctrl (0xE0): releasing the first must NOT release Ctrl yet.
    sched.press_control(1, &[0xE0, 0x06], &mut emitter).unwrap(); // Ctrl+C
    sched.press_control(2, &[0xE0, 0x19], &mut emitter).unwrap(); // Ctrl+V
    sched.release_control(1, &mut emitter).unwrap();
    assert!(sched.held_keys().contains(&0xE0));
    assert!(!sched.held_keys().contains(&0x06));

    sched.release_control(2, &mut emitter).unwrap();
    assert!(sched.held_keys().is_empty());
}

#[test]
fn firmware_image_parser_validates_header() {
    assert!(matches!(
        FirmwareImage::parse(&[0u8; 1024], FirmwareTarget::Device),
        Err(FirmwareError::TooSmall(_))
    ));

    let mut img = vec![0u8; 32 + 12_000];
    img[4] = 1; // mic target
    img[5..8].copy_from_slice(&[0x41, 0x00, 0x00]);
    img[28..32].copy_from_slice(&(12_000u32).to_le_bytes());
    img[32..].fill(1);

    let parsed = FirmwareImage::parse(&img, FirmwareTarget::Device).unwrap();
    assert_eq!(parsed.payload_length(), 12_000);
    assert_eq!(parsed.frames.len(), 12);
    assert_eq!(parsed.total_checksum(), 12_000);
    assert!(matches!(
        FirmwareImage::parse(&img, FirmwareTarget::Dongle),
        Err(FirmwareError::WrongTarget(1, FirmwareTarget::Dongle))
    ));
}

#[test]
fn hooks_install_uninstall_and_tracker_priority() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
    let claude = AGENTS.iter().find(|a| a.id == "claude-code").unwrap();

    let res =
        hooks::install_hooks(claude, dir.path(), "/usr/local/bin/open-au05", None, false).unwrap();
    assert_eq!(res.added, claude.events.len());
    let (installed, total, _) = hooks::hook_status(claude, dir.path());
    assert_eq!(installed, total);

    let unres = hooks::uninstall_hooks(claude, dir.path(), None, false).unwrap();
    assert_eq!(unres.removed, claude.events.len());
    let (installed_after, _, _) = hooks::hook_status(claude, dir.path());
    assert_eq!(installed_after, 0);

    let mut tracker = AgentTracker::default();
    tracker.update("claude-code", "s1", "working");
    tracker.update("codex", "s2", "attention");
    let (state, busy) = tracker.aggregate();
    assert_eq!(state, "attention");
    assert!(busy);

    tracker.update("claude-code", "s1", "notification");
    let (state2, busy2) = tracker.aggregate();
    assert_eq!(state2, "notification");
    assert!(!busy2);
}

#[test]
fn config_roundtrip_and_stt_json_parse() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let cfg = Config::default();
    cfg.save(Some(&path)).unwrap();
    let loaded = Config::load(Some(&path)).unwrap();
    assert_eq!(loaded.device.sleep_time, Some(3600));
    assert!(loaded.actions.get("talk").unwrap().dictate);

    assert_eq!(
        parse_stt_response(r#"{"text": " 안녕하세요 테스트입니다 "}"#).unwrap(),
        "안녕하세요 테스트입니다"
    );
    assert_eq!(
        parse_stt_response("plain output\n").unwrap(),
        "plain output"
    );
}
