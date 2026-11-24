//! The demo's two HTML newsletters, each with the text sent beside it.
//! Styles are inline and there are no pictures, so nothing is fetched
//! from the web when one is opened.

pub const FIELD_NOTES_TEXT: &str = "Field Notes #112: The quiet art of maintenance\n\n\
Most of what keeps a city working is somebody fixing something that \
broke last week. This issue: the people who repaint bridges, a short \
history of the pothole, and why the best tools are the ones you can \
repair yourself.\n\nRead in your browser: fieldnotes.example/112";

pub const FIELD_NOTES_HTML: &str = r#"<!doctype html>
<html><body style="margin:0;background:#f4f1ea;font-family:Georgia,serif;color:#2b2a27">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td align="center">
<table role="presentation" width="600" cellpadding="0" cellspacing="0" style="background:#ffffff;margin:24px 0">
<tr><td style="padding:28px 36px 8px;font:600 13px/1.4 Helvetica,Arial,sans-serif;letter-spacing:.12em;text-transform:uppercase;color:#8a6d3b">Field Notes &middot; No. 112</td></tr>
<tr><td style="padding:0 36px;font-size:30px;line-height:1.2">The quiet art of maintenance</td></tr>
<tr><td style="padding:16px 36px 0;font-size:17px;line-height:1.6">
<p>Most of what keeps a city working is somebody fixing something that broke last week.
Nobody cuts a ribbon for a repainted bridge, but without the painters it rusts through in a generation.</p>
<p>This week we spent a morning with the crew that keeps the Harbor Street bridge standing,
read up on the long and unglamorous history of the pothole, and asked a dozen readers which tool they have owned the longest.</p>
</td></tr>
<tr><td style="padding:8px 36px 0">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0" style="border-top:1px solid #e6e0d4">
<tr><td style="padding:16px 0 4px;font:600 15px/1.4 Helvetica,Arial,sans-serif">In this issue</td></tr>
<tr><td style="padding:4px 0;font-size:16px;line-height:1.5">&bull; Twelve coats of grey: a week with the bridge painters</td></tr>
<tr><td style="padding:4px 0;font-size:16px;line-height:1.5">&bull; The pothole, a history in four winters</td></tr>
<tr><td style="padding:4px 0 20px;font-size:16px;line-height:1.5">&bull; Readers' oldest tools, and why they kept them</td></tr>
</table></td></tr>
<tr><td style="padding:0 36px 32px"><a href="https://fieldnotes.example/112" style="display:inline-block;background:#2b2a27;color:#ffffff;text-decoration:none;font:600 14px/1 Helvetica,Arial,sans-serif;padding:12px 18px;border-radius:4px">Read the issue</a></td></tr>
</table>
<p style="font:12px/1.5 Helvetica,Arial,sans-serif;color:#8c867a;margin:0 0 24px">You subscribed to Field Notes Weekly. Unsubscribe any time.</p>
</td></tr></table>
</body></html>"#;

pub const FIGMENT_TEXT: &str = "New this month: Juniper Serif\n\n\
Six weights, true italics and old-style figures, made for long reading \
on screens. Try it free for 30 days in any project.\n\n\
figmentfonts.example/juniper";

pub const FIGMENT_HTML: &str = r#"<!doctype html>
<html><body style="margin:0;background:#101418;font-family:Helvetica,Arial,sans-serif;color:#e9ecef">
<table role="presentation" width="100%" cellpadding="0" cellspacing="0"><tr><td align="center">
<table role="presentation" width="600" cellpadding="0" cellspacing="0" style="margin:24px 0;background:#181e24;border-radius:8px">
<tr><td style="padding:32px 36px 4px;font-size:13px;letter-spacing:.14em;text-transform:uppercase;color:#7fd1b9">Figment Fonts &middot; New release</td></tr>
<tr><td style="padding:0 36px;font:400 46px/1.1 Georgia,serif;color:#ffffff">Juniper Serif</td></tr>
<tr><td style="padding:14px 36px 0;font:italic 22px/1.4 Georgia,serif;color:#c5ccd3">Six weights, true italics, old-style figures.</td></tr>
<tr><td style="padding:18px 36px 0;font-size:16px;line-height:1.6;color:#c5ccd3">
Juniper was drawn for long reading on screens: open counters, a generous x-height and a calm rhythm
that holds up from captions to headlines.</td></tr>
<tr><td style="padding:22px 36px 0">
<table role="presentation" cellpadding="0" cellspacing="0"><tr>
<td style="padding:10px 14px;border:1px solid #2c353e;font:300 20px Georgia,serif">Light</td>
<td style="padding:10px 14px;border:1px solid #2c353e;font:400 20px Georgia,serif">Regular</td>
<td style="padding:10px 14px;border:1px solid #2c353e;font:italic 400 20px Georgia,serif">Italic</td>
<td style="padding:10px 14px;border:1px solid #2c353e;font:700 20px Georgia,serif">Bold</td>
</tr></table></td></tr>
<tr><td style="padding:26px 36px 34px"><a href="https://figmentfonts.example/juniper" style="display:inline-block;background:#7fd1b9;color:#101418;text-decoration:none;font-weight:600;font-size:14px;padding:12px 20px;border-radius:4px">Try it free for 30 days</a></td></tr>
</table>
</td></tr></table>
</body></html>"#;
