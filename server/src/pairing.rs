//! Boot-time pairing: generate a fresh `xxxx-xxxx` code and print it (plus a
//! scannable ASCII QR) to the terminal so the user can type it into the app or
//! scan it with the tablet camera.
//!
//! The code is ephemeral — never stored, generated at startup, valid until the
//! server process exits. Restarting the server mints a new one.

use qrcode::{Color, QrCode};
use rand::Rng;

/// Unambiguous alphabet: no 0/O, 1/I/L, so the code is easy to type.
const ALPHABET: &[u8] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";

/// Generate a fresh pairing code, e.g. `K7XM-2PQ4`.
pub fn generate_code() -> String {
    let mut rng = rand::thread_rng();
    let mut pick = |n: usize| -> String {
        (0..n)
            .map(|_| {
                let i = rng.gen_range(0..ALPHABET.len());
                ALPHABET[i] as char
            })
            .collect()
    };
    format!("{}-{}", pick(4), pick(4))
}

/// Print the pairing banner + ASCII QR to stderr.
pub fn print_pairing(code: &str) {
    eprintln!();
    eprintln!("=====================================================");
    eprintln!("  NoIDE pairing required");
    eprintln!();
    eprintln!("  Connect from the app and enter this code:");
    eprintln!();
    eprintln!("          {}", code);
    eprintln!();
    eprintln!("  It is valid until this server exits and is never stored.");
    eprintln!("  (Use --token <value> for a fixed token, or --no-auth to");
    eprintln!("   allow unauthenticated connections.)");
    eprintln!("=====================================================");
    eprintln!();
    eprintln!("  Or scan this QR code with the app:");
    eprintln!();
    print_qr(code);
    eprintln!();
}

/// Render a QR code as terminal half-blocks (1 char per module, 2 module rows
/// per text line) with black glyphs on a white background for maximum
/// contrast on both light and dark terminal themes.
pub fn print_qr(data: &str) {
    let qr = QrCode::new(data).expect("failed to encode QR code");
    let size = qr.width();
    let colors = qr.to_colors();

    let is_dark = |x: usize, y: usize| -> bool { colors[y * size + x] == Color::Dark };

    // Quiet zone: 2 light modules on every side.
    let m = 2usize;
    let total = size + m * 2;
    let in_area = |x: usize, y: usize| -> Option<(usize, usize)> {
        if x >= m && x < total - m && y >= m && y < total - m {
            Some((x - m, y - m))
        } else {
            None
        }
    };

    // ANSI: black foreground on white background.
    let set = "\x1b[38;5;0;48;5;15m";
    let reset = "\x1b[0m";

    // Two module rows per printed line via half-block characters:
    //   ' ' both light, '▀' top dark, '▄' bottom dark, '█' both dark.
    let rows = total.div_ceil(2);
    for line in 0..rows {
        let top_y = line * 2;
        let bottom_y = top_y + 1;
        let mut buf = String::from(set);
        for x in 0..total {
            let top_dark = in_area(x, top_y)
                .map(|(mx, my)| is_dark(mx, my))
                .unwrap_or(false);
            let bottom_dark = if bottom_y < total {
                in_area(x, bottom_y)
                    .map(|(mx, my)| is_dark(mx, my))
                    .unwrap_or(false)
            } else {
                false
            };
            let ch = match (top_dark, bottom_dark) {
                (false, false) => ' ',
                (true, false) => '▀',
                (false, true) => '▄',
                (true, true) => '█',
            };
            buf.push(ch);
        }
        buf.push_str(reset);
        eprintln!("{}", buf);
    }
}
