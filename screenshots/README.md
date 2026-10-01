# Screenshots

The images used by the two READMEs. They are the first thing a visitor looks
at, and for a while neither README had any.

## What is in the gallery

| File | What it shows | Used by |
| --- | --- | --- |
| `processes-flat.png` | Main window, flat view: the LP / cores / CCD chips, the per-LP CPU histogram, and the process table with the per-CCD affinity bars | both READMEs |
| `processes-tree.png` | The same window in tree view, so the parent/child process hierarchy is visible | both READMEs |
| `rules.png` | The rule manager | both READMEs |
| `probalance.png` | The ProBalance panel | both READMEs |

## Capturing them again

Rules, if the screenshots ever need to be retaken:

- **Pick one theme and use it for every image.** The app defaults to dark
  (`cpum-theme` in localStorage) but the current set is light, because a white
  screenshot sits better on GitHub's white page. Mixing the two in one gallery
  looks like a mistake.
- **Pick one locale too.** The UI is bilingual and the images are not. The
  current set is `en-US`, which is why the captions are written that way.
- **Crop to the application window.** No desktop, no taskbar.
- **Redact nothing that is not personal.** A path with a Windows user name in it
  is fine — the point is that the tool shows real data. Blur anything you would
  not want published.
- **Keep them under ~300 KB each.** The current files are 118–285 KB at
  ~1880×1340; re-capture at a smaller window rather than recompressing.
- **One size per gallery.** `rules.png` is 1373×988 while the rest are ~1880×1340;
  at `width="420"` it renders slightly larger than its neighbours.
- **Two shots of the same dialog is one too many.** A second rule-manager state was
  captured and then dropped for exactly that reason.

After replacing a file, check that both `README.md` and `README.zh-CN.md` still
point at it, and that the captions still describe what is on screen.
