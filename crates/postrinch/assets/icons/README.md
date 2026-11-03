# Postrinch's icons

Made from `design/logo/icon.svg` (the paper-plane mark on a rounded square of the accent blue):

```bash
for n in 16 24 32 48 64 128 256 512; do
  rsvg-convert -w $n -h $n design/logo/icon.svg -o crates/postrinch/assets/icons/postrinch-$n.png
done
magick crates/postrinch/assets/icons/postrinch-{16,24,32,48,64,256}.png crates/postrinch/assets/icons/postrinch.ico
cp design/logo/icon.svg crates/postrinch/assets/icons/postrinch.svg
```

- `postrinch-256.png` is the window's icon.
- `postrinch.svg` and the PNGs are for a Linux desktop entry (hicolor theme sizes).
- `postrinch.ico` is for Windows.
- A macOS `.icns` comes with packaging.
