# Goko icon

Two interlocking pieces form a G: warm ivory (`#F4F1E9`) and orange (`#F47735`)
on charcoal (`#202220`). The transparent outer margin is part of the app icon.

- `icon.svg`: editable vector source, also displayed in the root README.
- `icon.png`: 1024 × 1024 export, included in the Linux archive for its launcher.
- `Goko.icns`: macOS icon with 16–1024 px representations, copied into both
  development and release bundles before signing.

After editing the SVG, regenerate and commit both exports on macOS:

```sh
bash apps/rune/assets/generate-macos.sh
```

The generator uses AppKit through Swift, `sips`, and `iconutil`. Normal builds
copy the committed exports and require no image conversion tools.
