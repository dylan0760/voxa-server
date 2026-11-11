/// Procedurally drawn application icon: a rounded blue square (green for the server)
/// with white sound bars. Shared by the window icon and the `.ico` generator.
pub fn icon_rgba(size: u32, server: bool) -> Vec<u8> {
    let mut px = vec![0u8; (size * size * 4) as usize];
    let s = size as f32;
    let radius = s * 0.22;
    let bars = [0.30f32, 0.55, 0.85, 0.55, 0.30];
    for y in 0..size {
        for x in 0..size {
            let fx = x as f32 + 0.5;
            let fy = y as f32 + 0.5;
            // Rounded square coverage.
            let dx = (fx - s / 2.0).abs() - (s / 2.0 - radius - 0.5);
            let dy = (fy - s / 2.0).abs() - (s / 2.0 - radius - 0.5);
            let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt() + dx.max(dy).min(0.0);
            let a = (radius - outside).clamp(0.0, 1.0);
            if a <= 0.0 {
                continue;
            }
            let t = fy / s;
            let (mut r, mut g, mut b) = if server {
                (40.0 + 20.0 * t, 165.0 - 30.0 * t, 95.0 - 10.0 * t)
            } else {
                (37.0 + 10.0 * t, 140.0 - 25.0 * t, 215.0 - 20.0 * t)
            };
            let bw = s * 0.09;
            let gap = s * 0.05;
            let total = bars.len() as f32 * bw + (bars.len() - 1) as f32 * gap;
            let x0 = s / 2.0 - total / 2.0;
            for (k, h) in bars.iter().enumerate() {
                let bx = x0 + k as f32 * (bw + gap);
                let bh = h * s * 0.6;
                if fx >= bx && fx < bx + bw && (fy - s / 2.0).abs() < bh / 2.0 {
                    r = 255.0;
                    g = 255.0;
                    b = 255.0;
                }
            }
            let i = ((y * size + x) * 4) as usize;
            px[i] = r as u8;
            px[i + 1] = g as u8;
            px[i + 2] = b as u8;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}
