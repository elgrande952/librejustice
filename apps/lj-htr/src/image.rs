//! Grey pixels resampled onto the fixed canvases the networks are planned for.

/// `rect` of the grey image `width` pixels wide, `[x, y, w, h]`, resampled
/// bilinearly to `dw` by `dh` at the top left of a white `cw` by `ch` canvas.
pub fn scaled(
    grey: &[u8],
    width: usize,
    rect: [f32; 4],
    dw: usize,
    dh: usize,
    cw: usize,
    ch: usize,
) -> Vec<f32> {
    let height = grey.len() / width;
    let [rx, ry, rw, rh] = rect;
    let (sx, sy) = (rw / dw as f32, rh / dh as f32);
    let at = |x: usize, y: usize| f32::from(grey[y.min(height - 1) * width + x.min(width - 1)]);
    let mut out = vec![255.0; cw * ch];
    for y in 0..dh.min(ch) {
        let fy = (ry + (y as f32 + 0.5) * sy - 0.5).max(0.0);
        let (y0, ty) = (fy as usize, fy.fract());
        for x in 0..dw.min(cw) {
            let fx = (rx + (x as f32 + 0.5) * sx - 0.5).max(0.0);
            let (x0, tx) = (fx as usize, fx.fract());
            let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
            let bottom = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
            out[y * cw + x] = top * (1.0 - ty) + bottom * ty;
        }
    }
    out
}
