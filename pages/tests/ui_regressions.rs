#[test]
fn playlist_import_dialog_owns_top_layer_and_checkbox_glyph() {
    let css = include_str!("../../nira/assets/css/library.css");
    assert!(css.contains(".content:has(.yt-downloader.open) { contain: none; }"));
    assert!(!css.contains(".content:has(.yt-downloader.open) { z-index:"));
    assert!(css.contains(".playlist-import-row input:checked::before"));
}

#[test]
fn desktop_webkit_does_not_partially_select_titles() {
    let css = include_str!("../../nira/assets/css/base.css");
    assert!(css.contains("-webkit-user-select: none;"));
}

#[test]
fn player_controls_publish_each_input_before_the_poller() {
    let bottombar = include_str!("../../components/src/bottombar.rs");
    let cover = include_str!("../../components/src/cover.rs");
    let player = include_str!("../../hooks/src/use_player.rs");
    let deferred_seek = ["if !*", "scrub_dragging.peek()"].concat();
    let reflected_seek = ["snapshot.write().", "position = target;"].concat();
    let reflected_volume = ["snapshot.write().", "volume = v;"].concat();

    assert!(!bottombar.contains(&deferred_seek));
    assert!(!cover.contains(&deferred_seek));
    assert!(bottombar.contains("player.seek(target);"));
    assert!(cover.contains("player.seek(target);"));
    assert!(player.contains(&reflected_seek));
    assert!(player.contains(&reflected_volume));
}

#[test]
fn custom_button_rows_own_space_instead_of_global_playback() {
    let rows = [
        include_str!("../src/parts.rs"),
        include_str!("../src/discover.rs"),
        include_str!("../src/library.rs"),
        include_str!("../../components/src/bottombar.rs"),
    ];
    for source in rows {
        assert!(source.contains("let is_space = key.to_string() == \" \";"));
        assert!(source.contains("e.stop_propagation();"));
    }

    let shell = include_str!("../../nira/src/main.rs");
    assert!(shell.contains("e.target.closest('button:not(.hotkey-bridge), [role=button]')"));
    assert!(!shell.contains("Enter only: Space stays the global play/pause bind."));

    let nested_controls = [
        include_str!("../src/parts.rs"),
        include_str!("../src/library.rs"),
        include_str!("../../components/src/bottombar.rs"),
    ];
    for source in nested_controls {
        assert!(source.contains("onkeydown: |e: KeyboardEvent| e.stop_propagation()"));
    }
}

#[test]
fn home_and_cover_respect_reduced_motion() {
    let home = include_str!("../../nira/assets/css/home.css");
    let cover = include_str!("../../nira/assets/css/cover.css");

    for css in [home, cover] {
        assert!(css.contains("@media (prefers-reduced-motion: reduce)"));
        assert!(css.contains("animation: none"));
        assert!(css.contains("transition: none"));
    }
    assert!(home.contains("transform: none"));
    assert!(cover.contains(".cover-overlay.closing"));
}

#[test]
fn narrow_layout_keeps_sidebar_and_player_geometry_in_sync() {
    let css = include_str!("../../nira/assets/css/responsive.css");

    assert!(css.contains("@media (max-width: 900px)"));
    assert!(css.contains("--sidebar-w: 72px"));
    assert!(css.contains("@media (max-width: 700px)"));
    assert!(css.contains("--player-h: 142px"));
    assert!(css.contains(".settings-shell"));
}

#[test]
fn audio_startup_failure_is_rendered_instead_of_panicking() {
    let source = include_str!("../../nira/src/main.rs");

    assert!(!source.contains(".expect(\"audio engine failed to start\")"));
    assert!(source.contains("Audio output could not start"));
    assert!(source.contains("The output device may be missing or busy."));
}

#[test]
fn settings_separate_music_from_providers_and_style_sessions() {
    let settings = include_str!("../src/settings/mod.rs");
    let connections = include_str!("../src/settings/connections.rs");
    let settings_css = include_str!("../../nira/assets/css/settings.css");
    let base_css = include_str!("../../nira/assets/css/base.css");

    assert!(settings.contains("SettingsTab::Providers"));
    assert!(settings.contains("MusicSettings {}"));
    assert!(settings.contains("ProviderSettings {}"));
    assert!(connections.contains("pub(super) fn MusicSettings()"));
    assert!(connections.contains("pub(super) fn ProviderSettings()"));
    assert_eq!(
        connections
            .matches("settings-input listen-together-code")
            .count(),
        2
    );
    assert!(settings_css.contains(".listen-together-code"));
    assert!(base_css.contains("height: 30px;\n  padding: 0 11px;\n  border-radius: var(--rs);"));
    assert!(base_css.contains("corner-shape: squircle;"));
}

#[test]
fn music_settings_expose_a_three_band_equalizer() {
    let connections = include_str!("../src/settings/connections.rs");
    let css = include_str!("../../nira/assets/css/settings.css");

    assert!(connections.contains("title: \"Equalizer\""));
    assert!(connections.contains("set_equalizer"));
    for band in ["Low", "Mid", "High"] {
        assert!(connections.contains(&format!("\"{band}\"")));
    }
    assert!(!connections.contains("disabled: !equalizer_enabled"));
    assert!(css.contains(".equalizer-grid"));
    assert!(css.contains(".equalizer-band"));
    assert!(css.contains(
        ".equalizer-toggle {\n  align-self: flex-start;\n  align-items: center;"
    ));
}

#[test]
fn search_state_is_installed_once_at_the_root() {
    let search = include_str!("../../hooks/src/use_search.rs");
    let hooks = include_str!("../../hooks/src/lib.rs");

    assert!(search.contains("pub(crate) fn install_search()"));
    assert!(search.contains(
        "pub fn use_search() -> UseSearch {\n    use_context::<UseSearch>()\n}"
    ));
    assert!(hooks.contains("use_search::install_search();"));
}

#[test]
fn search_results_have_an_internal_page_without_sidebar_navigation() {
    let components = include_str!("../../components/src/lib.rs");
    let pages = include_str!("../src/lib.rs");
    let shell = include_str!("../../nira/src/main.rs");
    let sidebar = include_str!("../../components/src/sidebar.rs");

    assert!(components.contains("Search,"));
    assert!(pages.contains("pub mod search;"));
    assert!(shell.contains("Section::Search"));
    assert!(!sidebar.contains("label: \"Search\""));
}

#[test]
fn overlay_submit_opens_search_page_instead_of_starting_playback() {
    let overlay = include_str!("../src/search_overlay.rs");
    let shell = include_str!("../../nira/src/main.rs");

    assert!(overlay.contains("on_search: EventHandler<()>"));
    assert!(overlay.contains("on_search.call(())"));
    assert!(!overlay.contains("queue.play_list(list, 0)"));
    assert!(overlay.contains("Enter opens the full page."));
    assert!(!overlay.contains("Enter plays"));
    assert!(shell.contains("section.set(Section::Search)"));
}
