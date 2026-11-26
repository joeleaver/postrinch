# Logo

| File | What it is |
|---|---|
| `plane.svg` | The paper-plane mark on its own |
| `envelope.svg` | The envelope mark |
| `icon.svg` | The app icon: the plane on a rounded square of the accent blue (#2456b8) |
| `google-oauth-logo.svg` | `icon.svg` with the square full-bleed (no rounded corners), for Google |
| `google-oauth-logo-120.png` | The logo uploaded to Google's OAuth consent screen (Google Cloud console › Branding) |
| `preview.png` | A preview of the marks |

Google asks for a square logo of 120×120 pixels. The PNG is made from the SVG, flattened onto
the blue so it has no transparency:

```bash
rsvg-convert -w 120 -h 120 design/logo/google-oauth-logo.svg -o /tmp/oauth-logo.png
magick /tmp/oauth-logo.png -background '#2456b8' -alpha remove -alpha off \
  design/logo/google-oauth-logo-120.png
```

If `icon.svg` changes, change `google-oauth-logo.svg` to match, remake the PNG, and upload it to
Google again (a new logo may need Google to verify the app again).
