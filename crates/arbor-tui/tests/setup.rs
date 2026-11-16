use arbor_config::{RoleConfig, Roles};
use arbor_tui::{KeyStorage, SetupDialog, SetupOutcome, draw_setup};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn press(dialog: &mut SetupDialog, code: KeyCode) -> Option<SetupOutcome> {
    dialog.handle_key(KeyEvent::new(code, KeyModifiers::NONE))
}

fn type_text(dialog: &mut SetupDialog, typed: &str) {
    for typed_char in typed.chars() {
        assert_eq!(press(dialog, KeyCode::Char(typed_char)), None);
    }
}

fn clear_field(dialog: &mut SetupDialog) {
    for _ in 0..60 {
        press(dialog, KeyCode::Backspace);
    }
}

fn screen(dialog: &SetupDialog) -> String {
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal.draw(|frame| draw_setup(frame, dialog)).unwrap();
    terminal.backend().to_string()
}

fn custom_roles() -> Roles {
    let mut roles = Roles::recommended();
    roles.writer = RoleConfig {
        model: "x/writer".to_owned(),
        temperature: 0.2,
    };
    roles
}

#[test]
fn a_typed_key_is_saved_or_kept_for_the_session() {
    for (storage_key, storage) in [
        ('s', KeyStorage::UserConfig),
        ('o', KeyStorage::SessionOnly),
    ] {
        let mut dialog = SetupDialog::api_key(None);
        assert_eq!(press(&mut dialog, KeyCode::Enter), None);
        type_text(&mut dialog, "sk-or-new1234");
        assert_eq!(press(&mut dialog, KeyCode::Enter), None);
        assert_eq!(
            press(&mut dialog, KeyCode::Char(storage_key)),
            Some(SetupOutcome::ApiKey {
                api_key: "sk-or-new1234".to_owned(),
                storage,
            })
        );
    }
}

#[test]
fn the_key_is_masked_and_a_refusal_is_shown() {
    let mut dialog = SetupDialog::api_key(Some("OpenRouter rejected the API key".to_owned()));
    dialog.paste("  sk-or-secret9876\n");
    let shown = screen(&dialog);
    assert!(shown.contains("9876"));
    assert!(!shown.contains("secret"));
    assert!(shown.contains("OpenRouter rejected the API key"));
}

#[test]
fn esc_leaves_the_key_dialog_and_backs_out_of_the_storage_choice() {
    let mut dialog = SetupDialog::api_key(None);
    type_text(&mut dialog, "sk");
    press(&mut dialog, KeyCode::Enter);
    assert_eq!(press(&mut dialog, KeyCode::Esc), None);
    type_text(&mut dialog, "x");
    assert_eq!(press(&mut dialog, KeyCode::Esc), Some(SetupOutcome::Quit));
}

#[test]
fn keys_are_ignored_while_checking() {
    let mut dialog = SetupDialog::models(custom_roles(), vec![]);
    dialog.set_checking(true);
    assert_eq!(press(&mut dialog, KeyCode::Char('r')), None);
    assert!(screen(&dialog).contains("Probing the models"));
    dialog.set_checking(false);
    assert_eq!(
        press(&mut dialog, KeyCode::Char('r')),
        Some(SetupOutcome::Roles(Roles::recommended()))
    );
}

#[test]
fn the_models_dialog_shows_the_current_and_recommended_roles_and_problems() {
    let dialog = SetupDialog::models(
        custom_roles(),
        vec!["x/writer refused a call at temperature 0.2".to_owned()],
    );
    let shown = screen(&dialog);
    assert!(shown.contains("x/writer  at 0.2"));
    assert!(shown.contains(&Roles::recommended().writer.model));
    assert!(shown.contains("refused a call"));
}

#[test]
fn setting_roles_by_hand_starts_from_the_current_config() {
    let mut dialog = SetupDialog::models(custom_roles(), vec![]);
    assert_eq!(press(&mut dialog, KeyCode::Char('m')), None);
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        Some(SetupOutcome::Roles(custom_roles()))
    );

    let mut dialog = SetupDialog::models(custom_roles(), vec![]);
    press(&mut dialog, KeyCode::Char('m'));
    press(&mut dialog, KeyCode::Up);
    clear_field(&mut dialog);
    type_text(&mut dialog, "0.7");
    press(&mut dialog, KeyCode::Up);
    clear_field(&mut dialog);
    dialog.paste("y/writer");
    let mut expected = custom_roles();
    expected.writer = RoleConfig {
        model: "y/writer".to_owned(),
        temperature: 0.7,
    };
    assert_eq!(
        press(&mut dialog, KeyCode::Enter),
        Some(SetupOutcome::Roles(expected))
    );
}

#[test]
fn typed_roles_with_problems_stay_open_and_list_them() {
    let mut dialog = SetupDialog::models(Roles::recommended(), vec![]);
    press(&mut dialog, KeyCode::Char('m'));
    clear_field(&mut dialog);
    press(&mut dialog, KeyCode::Tab);
    clear_field(&mut dialog);
    type_text(&mut dialog, "warm");
    press(&mut dialog, KeyCode::Tab);
    press(&mut dialog, KeyCode::Tab);
    clear_field(&mut dialog);
    type_text(&mut dialog, "1.5");
    assert_eq!(press(&mut dialog, KeyCode::Enter), None);
    let shown = screen(&dialog);
    assert!(shown.contains("the Planner needs a model"));
    assert!(shown.contains("the Planner temperature \"warm\" is not a number"));
    assert!(shown.contains("the Scaffolder temperature 1.5 is outside 0–1"));

    assert_eq!(press(&mut dialog, KeyCode::Esc), None);
    assert_eq!(press(&mut dialog, KeyCode::Esc), Some(SetupOutcome::Quit));
}
