# Local Arch package

This recipe packages the current checkout, including uncommitted changes. It is
for local testing, not an AUR submission. From this directory, as a regular user
with `base-devel` installed:

```sh
makepkg -si
```

The `gurk-local-aliases` package installs `/usr/bin/gurk` and the project README
and license. It provides `gurk` and conflicts with `gurk`, `gurk-git` and
`gurk-bin`, allowing pacman to replace an existing installation. Configuration,
databases, registration and private attachments are not part of the package.

Before first launch against existing history, close other gurk instances and
copy the data directory. Database migrations run automatically; older application
versions may not open the new schema. Reverting can require the saved database
copy as well as the old executable.

The [main README](../../README.md) covers features, shortcuts, settings, image
viewers, offline dictionaries, attachment workflows and experimental Secure Backup
import. Existing configurations stay readable. Inline previews, the shortcut bar
and ASCII emoticon conversion are opt-in; JPEG/PNG metadata cleaning and spelling
checks are enabled by default.

To return to Arch's upstream package:

```sh
sudo pacman -S gurk
```

That installs original gurk without these extensions; database compatibility must
still be considered. Public binary distribution also needs the corresponding
source and applicable third-party notices. This recipe includes the root AGPL
text but does not assemble all dependency notices; see the
[license review](../../docs/LICENSE_REVIEW.md).
