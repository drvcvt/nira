# Spotify Playlist Import Plan

**Goal:** Import the connected user's readable Spotify playlists into Nira's existing local playlists from Library → Playlists.

## Constraints

- Keep the change on the public branch and free of Qobuz references.
- Use Spotify's current `/me/playlists` and `/playlists/{id}/items` endpoints with 50-item pagination.
- Import owned and collaborative playlists; report followed playlists whose items Spotify forbids as skipped.
- Skip episodes, local files, and unavailable items without failing the whole import.
- Never overwrite an already imported local playlist.
- Add no dependencies or new persistence format.
- Verify only through `anvil tests` and `anvil check`.

## Tasks

- [x] Add one provider test for retaining tracks while dropping unsupported playlist items.
- [x] Add paginated playlist loading to `provider-spotify/src/lib.rs`.
- [x] Add one store test for deduplicated, non-destructive re-import.
- [x] Add a single-write Spotify import method to `hooks/src/use_playlists.rs`.
- [x] Add an Import from Spotify button and compact result status to `pages/src/library.rs`.
- [x] Run `anvil tests`, `anvil check`, and `git diff --check`.
