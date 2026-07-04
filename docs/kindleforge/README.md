# KindleForge submission

The `KindleButtonMapper/` package here is a thin bootstrap: its `install.sh` /
`uninstall.sh` fetch and run `docs/kindleforge/install.sh` / `uninstall.sh` from
this repo at `master`. Those two scripts hold the real logic and download the
release artifact built by `.github/workflows/build-arm.yml`. Once the bootstrap
is merged into KindleForge, install/uninstall changes ship by editing the
scripts in this repo — no further KindleForge PR needed.

Cut a release first so the tarball exists:

```bash
git tag v0.1.0
git push origin v0.1.0
# CI builds + publishes kindle-button-mapper-armv7.tar.gz to GitHub Releases
```

Update the package on https://github.com/KindleTweaks/Repository:

```bash
git clone git@github.com:YOUR_USER/Repository.git   # your fork
cd Repository
cp /path/to/kindle-button-mapper-rs/docs/kindleforge/KindleButtonMapper/*.sh \
  KindleButtonMapper/
```

Registry entry (already live upstream; include only when first adding the package):

```json
    {
        "name": "Kindle Button Mapper",
        "uri": "KindleButtonMapper",
        "description": "Map gamepad/remote/keyboard buttons to KOReader, key events, or custom scripts",
        "author": "Lucas Zampieri",
        "ABI": ["hf", "sf"],
        "dependencies": [],
        "tags": ["UTILITY"]
    }
```

```bash
git checkout -b button-mapper-bootstrap
git add KindleButtonMapper
git commit -s -m "KindleButtonMapper: bootstrap install from upstream repo"
git push origin button-mapper-bootstrap
# open PR on github.com
```

## Pre-PR checklist

- [ ] Tag pushed and CI release succeeded (tarball at `releases/latest/download/kindle-button-mapper-armv7.tar.gz`)
- [ ] `docs/kindleforge/install.sh` reachable at the raw `master` URL the bootstrap curls
- [ ] `install.sh` runs cleanly on a fresh Kindle (installs the upstart job, autostarts on boot)
- [ ] `uninstall.sh` cleans up fully (removes `/etc/upstart/kindle-button-mapper.conf`)
