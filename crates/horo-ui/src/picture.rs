//! Call pictures drawn on Linux: an I420 frame becomes a gpui image (BGRA),
//! converted once per picture, not once per redraw. gpui keeps every image
//! it has drawn in its sprite atlas until told otherwise, and a camera
//! brings thirty new ones a second, so each one replaced is let go of once
//! the frame that drew it is done.

use std::{cell::RefCell, collections::HashMap, sync::Arc};

use gpui::{App, RenderImage};
use horo_rtc::video::Frame;

thread_local! {
    /// The image shown for each key (someone's camera or screen, our own
    /// camera), and the picture it was made from.
    static SHOWN: RefCell<HashMap<String, (u64, Arc<RenderImage>)>> = RefCell::new(HashMap::new());
}

/// The image for `key`'s picture number `seq`, made from `frame` (flipped
/// left to right if `mirror`) when it's a new one.
pub fn image(key: &str, seq: u64, frame: &Frame, mirror: bool, cx: &mut App) -> Option<Arc<RenderImage>> {
    let (image, old) = SHOWN.with_borrow_mut(|shown| match shown.get(key) {
        Some((shown_seq, image)) if *shown_seq == seq => Some((image.clone(), None)),
        _ => {
            let image = Arc::new(render(frame, mirror)?);
            let old = shown.insert(key.to_owned(), (seq, image.clone())).map(|(_, old)| old);
            Some((image, old))
        }
    })?;
    if let Some(old) = old {
        let_go(old, cx);
    }
    Some(image)
}

/// `key` has nothing to show any more: let its last image go.
pub fn forget(key: &str, cx: &mut App) {
    if let Some((_, old)) = SHOWN.with_borrow_mut(|shown| shown.remove(key)) {
        let_go(old, cx);
    }
}

/// Let every image go (the call ended).
pub fn forget_all(cx: &mut App) {
    for (_, (_, old)) in SHOWN.with_borrow_mut(std::mem::take) {
        let_go(old, cx);
    }
}

/// Take `image` out of every window's atlas, after the frame being drawn
/// (the window drawing now isn't in the app's list until it's done).
fn let_go(image: Arc<RenderImage>, cx: &mut App) {
    cx.defer(move |cx| cx.drop_image(image, None));
}

/// A video-range I420 frame as a BGRA image (BT.601).
pub(crate) fn render(frame: &Frame, mirror: bool) -> Option<RenderImage> {
    let (w, h) = (frame.width as usize, frame.height as usize);
    if w == 0 || h == 0 {
        return None;
    }
    let cw = w.div_ceil(2);
    let (ys, us, vs) = (frame.y(), frame.u(), frame.v());
    let mut out = vec![255u8; w * h * 4];
    for row in 0..h {
        let (luma, chroma) = (&ys[row * w..(row + 1) * w], (row / 2) * cw);
        let line = &mut out[row * w * 4..(row + 1) * w * 4];
        for col in 0..w {
            let src = if mirror { w - 1 - col } else { col };
            let c = i32::from(luma[src]) - 16;
            let d = i32::from(us[chroma + src / 2]) - 128;
            let e = i32::from(vs[chroma + src / 2]) - 128;
            let r = (298 * c + 409 * e + 128) >> 8;
            let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
            let b = (298 * c + 516 * d + 128) >> 8;
            let px = &mut line[col * 4..col * 4 + 4];
            px[0] = b.clamp(0, 255) as u8;
            px[1] = g.clamp(0, 255) as u8;
            px[2] = r.clamp(0, 255) as u8;
        }
    }
    let buffer = image::RgbaImage::from_raw(w as u32, h as u32, out)?;
    Some(RenderImage::new(vec![image::Frame::new(buffer)]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn video_range_black_and_white_come_out_full_range() {
        let mut frame = Frame::black(2, 2);
        frame.data[0] = 235;
        let image = render(&frame, false).unwrap();
        let bytes = image.as_bytes(0).unwrap();
        assert_eq!(&bytes[0..4], &[255, 255, 255, 255]);
        assert_eq!(&bytes[4..8], &[0, 0, 0, 255]);
        // Mirrored, the white pixel moves to the other side.
        let mirrored = render(&frame, true).unwrap();
        assert_eq!(&mirrored.as_bytes(0).unwrap()[4..8], &[255, 255, 255, 255]);
    }
}
