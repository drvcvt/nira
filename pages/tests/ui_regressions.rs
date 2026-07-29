#[test]
fn playlist_import_dialog_owns_top_layer_and_checkbox_glyph() {
    let css = include_str!("../../nira/assets/css/library.css");
    assert!(css.contains(".content:has(.yt-downloader.open)"));
    assert!(css.contains(".playlist-import-row input:checked::before"));
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
