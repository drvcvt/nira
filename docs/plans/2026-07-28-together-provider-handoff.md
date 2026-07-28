# Listen-along Provider Handoff Plan

**Goal:** Keep a public follower aligned when the private host moves through an unavailable Qobuz track, and prevent the follower from replacing host playback locally.

## Tasks

- [x] Publish `Stopped` while the host is between loaded sources so a new queue entry never carries the previous song's playback id.
- [x] Reject follower-originated play, next, previous, stop, and natural queue advance while keeping host adoption able to bypass that gate.
- [x] Stop the follower on an unavailable host track, then adopt the next shared Spotify/SoundCloud target normally.
- [x] Keep the shared sync logic provider-neutral on the public branch.
- [x] Run the focused regressions, `anvil tests`, `anvil check`, and `git diff --check`.
