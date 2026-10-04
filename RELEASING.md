# Making a release

For the maintainers: how a version of Pipedeck gets out.

1. Set `version` in the root `Cargo.toml`, and move the changelog's
   *Unreleased* section under the new version.
2. Commit, and tag that commit: `git tag -a v0.2.0 -m "Pipedeck 0.2.0"`.
3. Push the commit, then the tag: `git push origin main v0.2.0`.

The tag builds the .deb, the .rpm and the AppImage, and puts them on a
release whose notes are that version's section of the changelog. The
workflow stops if the tag and `Cargo.toml` disagree.
