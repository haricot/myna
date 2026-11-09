# `dev/` — developer scripts

Everything here is a tool for working *on* Myna. Nothing here is shipped.

## Benchmarking lives elsewhere

There is one benchmarking tool, `myna.benchmarker`
(`server/src/myna/benchmarker/`): corpus builders, the clip scorer, the snap
sweep, the aggregator and the environment guard. Run it in-tree with the
`make bench-*` targets, or pack it for external testers with `make build-bench`
(`myna-bench.pyz`). It has no dependency on this directory, which is what lets
the same code run from a checkout and from a tester's download.

```shell
make bench-check          # is this machine fit to benchmark on?
make bench-plan           # every row the sweep would produce; no root
make bench-run            # the sweep (sudo: installs and purges snaps)
make bench-aggregate      # re-print the table
```

Only the two configs stayed here:

| File | What it is |
| --- | --- |
| `bench.yaml` | The in-repo sweep: which snaps, which axes. Same format as the example below; its `files:` globs point into the `*-snap/` directories of this tree. |
| `bench.yaml.example` | The template testers edit, pointing at downloaded artefacts instead. Carries the annotated schema both files share. |

## What is here

| Script | What it does |
| --- | --- |
| `generate_fixtures.py` | Synthetic espeak fixture tier for the offline test suite. Not a WER corpus — the synthetic voice is out of distribution and scores misleadingly across architectures. |
| `synth_cues.py` | Synthesizes the daemon's sound sets (start, stop and error each) into `client/myna-desktop/sounds/<set>/`; `uv run dev/synth_cues.py`. Seeded and bit-exact, so a rerun reproduces the committed `.oga` files. |
| `fetch_funasr_model.py`, `parakeet/fetch_parakeet_onnx.py` | Fetch and stage model weights into a snap directory. Driven by the `snap-*` make targets. |
| `parakeet/collapse_probe.py` | Sliding-window encoder collapse probe: the methodology behind the collapse figures in `myna.testbed.parakeet`. |
| `model-pin.sh`, `lint-packages.sh`, `version.sh`, `stage-version.sh`, `publish-snap.sh`, `release-notes.sh` | Snap staging, packaging checks, the git-derived version every build carries, store upload (`make publish-<snap>`) and the release-notes draft (`make release-notes`). |
| `spread-build.sh`, `spread-image.sh` | Confined end-to-end (spread) harness. |
| `adapter_coverage.py`, `coverage_populations.py`, `coverage_lib.py`, `gjs_coverage.py`, `patch_cov.py` | Coverage reports and gates behind `make coverage`. |
| `exercise.sh`, `gated-tests.sh`, `transcribe.py`, `capabilities.py` | Manual drivers for a running server. |
| `nested-session.sh` | Runs a command (a headless GNOME Shell, a GTK probe) on a private session bus and a private `XDG_RUNTIME_DIR`, so its AT-SPI bus and Wayland socket cannot clobber the host desktop's. `NESTED_SHARE_AUDIO=1` links the host's PipeWire in. |
| `try-desktop.sh` | A desktop VM (GNOME, Xubuntu or Kubuntu) running HEAD's snap and deb with the host's audio; `make try-desktop`, see e2e-tests/README.md. |
| `i18n.sh`, `ibus-doctor.sh` | Translation templates, IBus diagnosis. |
| `purge.sh` | Remove every trace of a Myna install (snaps, snapd flag, shortcuts, dconf, stray files) to test a fresh install; `--deb` also purges myna-config. |

## Experimental anchored dictation check (Firefox / AT-SPI)

`atspi_anchor_probe.py` is the manual, read-only-by-default proof of concept.
`atspi_anchor_firefox.py` automates its **real insertion** counterpart on the
existing graphical session. It launches a dedicated Firefox with a throwaway
WebDriver profile, enables Firefox accessibility, opens a generated textarea
with a unique AT-SPI accessible name, focuses it, types before a saved anchor
and checks both the AT-SPI write location and whether the user's editing caret
stayed put or can be restored. If Firefox moves the editing caret as a side
effect of `EditableText.insertText`, the probe calls AT-SPI
`Text.setCaretOffset` to return it to the saved editing position, then checks
the returned caret in **both** Firefox/WebDriver and AT-SPI and checks the text
again. A positive return from the API without a real caret move is a failure.
This is a **two-step restoration experiment**, not simultaneous independent
carets: a brief cursor jump can remain visible, and concurrent typing between
the insertion and restoration remains a race to solve before production use.
It never targets a real document or browser profile.
The unique name is necessary because Firefox/Wayland may report no AT-SPI
`STATE_FOCUSED` despite WebDriver confirming focus; the test does not rely on
that state for its scratch-field lookup. A failure reports only accessibility
tree counts, never the names or contents of other application fields.

Install `geckodriver` once (Arch/Manjaro: `sudo pacman -S geckodriver`), and
run with the system Python that imports `pyatspi`:

```shell
/usr/bin/python3 dev/atspi_anchor_firefox.py \
  --firefox /absolute/path/to/firefox
```

This is **not** a Myna injection backend or a completed feature. A compositor
may prevent a newly launched window from acquiring OS focus under Wayland; in
that case the test fails with a focus diagnostic without writing elsewhere.
It must run in the actual logged-in desktop, not headlessly or on a private
AT-SPI bus. Unit tests: `python3 -m unittest discover -s dev -p
'test_atspi_anchor*.py' -v`. Live Firefox behavior still requires the desktop
run above; the hermetic tests cannot establish real AT-SPI support.
