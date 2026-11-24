//! A small, valid one-page PDF of lines of text, for the receipts and
//! quotes the demo's mail carries: an attachment that opens, and whose
//! text an agent can read.

/// A US Letter page with `lines` in Helvetica, top to bottom.
pub fn document(lines: &[&str]) -> Vec<u8> {
    let mut text = String::from("BT\n/F1 12 Tf\n16 TL\n72 720 Td\n");
    for line in lines {
        text.push_str(&format!("({}) Tj T*\n", escape(line)));
    }
    text.push_str("ET\n");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] \
         /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>"
            .to_owned(),
        format!("<< /Length {} >>\nstream\n{text}endstream", text.len()),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::with_capacity(objects.len());
    for (n, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{object}\nendobj\n", n + 1));
    }
    let xref = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

/// A line as a PDF string holds it: ASCII only (the demo writes none
/// other), with its parentheses and backslashes escaped.
fn escape(line: &str) -> String {
    debug_assert!(line.is_ascii(), "the demo's PDFs are ASCII: {line:?}");
    line.replace('\\', "\\\\")
        .replace('(', "\\(")
        .replace(')', "\\)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cross_reference_points_at_each_object() {
        let pdf = String::from_utf8(document(&["Receipt (paid)", "Total: $42.00"])).unwrap();
        assert!(pdf.starts_with("%PDF-1.4\n"));
        assert!(pdf.contains("(Receipt \\(paid\\)) Tj"));
        let xref: usize = pdf
            .rsplit("startxref\n")
            .next()
            .and_then(|s| s.lines().next())
            .and_then(|s| s.parse().ok())
            .unwrap();
        assert!(pdf[xref..].starts_with("xref\n0 6\n"));
        let entries: Vec<usize> = pdf[xref..]
            .lines()
            .skip(3)
            .take(5)
            .map(|l| l[..10].parse().unwrap())
            .collect();
        for (n, offset) in entries.into_iter().enumerate() {
            assert!(pdf[offset..].starts_with(&format!("{} 0 obj", n + 1)));
        }
    }
}
