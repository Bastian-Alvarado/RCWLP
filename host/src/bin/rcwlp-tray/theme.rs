//! SimonSays's default look for the tray app's windows: rose on dark zinc, in Montserrat.
//!
//! Windows draws its standard controls light, so this paints the window backgrounds, frames the
//! text fields, colours the labels and draws the buttons itself. The fonts are built into the
//! program, so they needn't be installed.

use std::collections::HashMap;
use std::mem;

use native_windows_gui as nwg;
use winapi::shared::minwindef::{LRESULT, TRUE};
use winapi::shared::windef::{COLORREF, HBRUSH, HDC, HGDIOBJ, HWND, POINT, RECT};
use winapi::um::wingdi::{
    CreatePen, CreateSolidBrush, GetStockObject, NULL_PEN, PS_SOLID, RGB, RoundRect, SelectObject, SetBkColor,
    SetBkMode, SetTextColor, TRANSPARENT,
};
use winapi::um::winuser::{
    BM_GETSTATE, BST_PUSHED, BeginPaint, DT_CENTER, DT_SINGLELINE, DT_VCENTER, DrawTextW, EndPaint, FillRect,
    GetClientRect, GetWindowRect, GetWindowTextW, MapWindowPoints, PAINTSTRUCT, SendMessageW, WM_CTLCOLOREDIT,
    WM_CTLCOLORSTATIC, WM_ERASEBKGND, WM_PAINT,
};

/// The page, as in SimonSays.
pub const PAGE: [u8; 3] = [0x29, 0x29, 0x29];
/// Text fields: zinc-950 with a zinc-800 edge.
pub const FIELD: [u8; 3] = [0x09, 0x09, 0x0B];
const FIELD_EDGE: [u8; 3] = [0x27, 0x27, 0x2A];
/// Body text, zinc-300.
pub const TEXT: [u8; 3] = [0xD4, 0xD4, 0xD8];
pub const ROSE: [u8; 3] = [0xF4, 0x3F, 0x5E];
const WHITE: [u8; 3] = [0xFF, 0xFF, 0xFF];

/// A button's fill when idle, under the mouse and pressed, and its edge.
struct ButtonLook {
    idle: [u8; 3],
    hot: [u8; 3],
    pressed: [u8; 3],
    edge: Option<[u8; 3]>,
}

/// SimonSays's primary button: rose.
const PRIMARY: ButtonLook = ButtonLook {
    idle: ROSE,
    hot: [0xFB, 0x71, 0x85],
    pressed: [0xE1, 0x1D, 0x48],
    edge: None,
};

/// SimonSays's secondary button: zinc-800 with a zinc-700 edge.
const SECONDARY: ButtonLook = ButtonLook {
    idle: [0x27, 0x27, 0x2A],
    hot: [0x3F, 0x3F, 0x46],
    pressed: [0x18, 0x18, 0x1B],
    edge: Some([0x3F, 0x3F, 0x46]),
};

/// BM_GETSTATE's "the mouse is over it", which winapi doesn't name.
const BST_HOT: u32 = 0x0200;

/// Corner diameter of fields and buttons, in pixels.
const ROUND: i32 = 14;
/// Space between a text field's frame and its text.
const FIELD_PAD: (i32, i32) = (10, 7);

fn rgb(c: [u8; 3]) -> COLORREF {
    RGB(c[0], c[1], c[2])
}

/// Montserrat in the weights SimonSays uses, and JetBrains Mono for the log.
pub struct Fonts {
    /// The font files; Windows keeps its own copy, but they live as long as the fonts anyway.
    _files: Vec<Vec<u8>>,
    pub heading: nwg::Font,
    pub button: nwg::Font,
    pub code: nwg::Font,
    pub mono: nwg::Font,
}

impl Fonts {
    /// Loads the built-in fonts for this program only, and makes Montserrat the default.
    pub fn load() -> Fonts {
        let mut files: Vec<Vec<u8>> = [
            &include_bytes!("../../../assets/fonts/montserrat_medium.ttf")[..],
            include_bytes!("../../../assets/fonts/montserrat_extrabold.ttf"),
            include_bytes!("../../../assets/fonts/montserrat_black.ttf"),
            include_bytes!("../../../assets/fonts/jetbrains_mono.ttf"),
        ]
        .iter()
        .map(|f| f.to_vec())
        .collect();
        for file in &mut files {
            // Without it the fonts below fall back to the system font: plainer, but still fine.
            let _ = nwg::Font::add_memory_font(file);
        }
        let font = |family: &str, weight: u32, px: u32| {
            let mut f = nwg::Font::default();
            let _ = nwg::Font::builder()
                .family(family)
                .weight(weight)
                .size_absolute(px)
                .build(&mut f);
            f
        };
        let fonts = Fonts {
            heading: font("Montserrat Black", 900, 12),
            button: font("Montserrat ExtraBold", 800, 12),
            code: font("Montserrat Black", 900, 22),
            mono: font("JetBrains Mono", 400, 13),
            _files: files,
        };
        let mut default = nwg::Font::default();
        if nwg::Font::builder()
            .family("Montserrat Medium")
            .weight(500)
            .size_absolute(14)
            .build(&mut default)
            .is_ok()
        {
            nwg::Font::set_global_default(Some(default));
        }
        fonts
    }
}

/// The program's own icon, built in from assets/rcwlp.ico by build.rs, at the size Windows uses
/// for small icons (the tray, title bars) or large ones, so each gets the picture drawn for it.
pub fn app_icon(big: bool) -> nwg::Icon {
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::winuser::{
        GetSystemMetrics, IDI_APPLICATION, IMAGE_ICON, LoadIconW, LoadImageW, MAKEINTRESOURCEW, SM_CXICON, SM_CXSMICON,
    };
    let mut icon = nwg::Icon::default();
    // Safety: plain Win32 calls; the resource id is the one winresource gives the icon.
    unsafe {
        let size = GetSystemMetrics(if big { SM_CXICON } else { SM_CXSMICON });
        let handle = LoadImageW(
            GetModuleHandleW(std::ptr::null()),
            MAKEINTRESOURCEW(1),
            IMAGE_ICON,
            size,
            size,
            0,
        );
        icon.handle = if handle.is_null() {
            LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) as _
        } else {
            handle
        };
    }
    icon
}

/// Gives a window SimonSays's dark page: a dark title bar, the page colour behind everything,
/// rounded dark frames around `fields`, and `labels` in their colours.
pub fn style_window(
    window: &nwg::Window,
    fields: &[&nwg::ControlHandle],
    labels: &[(&nwg::Label, [u8; 3])],
) -> Vec<nwg::RawEventHandler> {
    let hwnd = window.handle.hwnd().expect("a window");
    dark_title_bar(hwnd);
    // nwg sets only the small icon; Alt+Tab and the taskbar want the large one.
    unsafe {
        use winapi::um::winuser::{ICON_BIG, WM_SETICON};
        SendMessageW(hwnd, WM_SETICON, ICON_BIG as usize, app_icon(true).handle as isize);
    }
    // Created once and kept for the life of the program, like the windows.
    let (page, field) = unsafe { (CreateSolidBrush(rgb(PAGE)), CreateSolidBrush(rgb(FIELD))) };
    let edge = unsafe { CreatePen(PS_SOLID as i32, 1, rgb(FIELD_EDGE)) };
    let mut handlers: Vec<nwg::RawEventHandler> = fields.iter().map(|f| hide_edges(f, field)).collect();
    let fields: Vec<HWND> = fields.iter().filter_map(|h| h.hwnd()).collect();
    let labels: HashMap<usize, COLORREF> = labels
        .iter()
        .filter_map(|(l, c)| Some((l.handle.hwnd()? as usize, rgb(*c))))
        .collect();
    let field_hwnds = fields.clone();

    let window = nwg::bind_raw_event_handler(&window.handle, 0x10001, move |hwnd, msg, w, l| unsafe {
        match msg {
            WM_ERASEBKGND => {
                let dc = w as HDC;
                let mut client: RECT = mem::zeroed();
                GetClientRect(hwnd, &mut client);
                FillRect(dc, &client, page);
                for &f in &field_hwnds {
                    let mut r = rect_in(hwnd, f);
                    r.left -= FIELD_PAD.0;
                    r.right += FIELD_PAD.0;
                    r.top -= FIELD_PAD.1;
                    r.bottom += FIELD_PAD.1;
                    let old_pen = SelectObject(dc, edge as HGDIOBJ);
                    let old_brush = SelectObject(dc, field as HGDIOBJ);
                    RoundRect(dc, r.left, r.top, r.right, r.bottom, ROUND, ROUND);
                    SelectObject(dc, old_brush);
                    SelectObject(dc, old_pen);
                }
                Some(1)
            }
            // Read-only text fields ask with WM_CTLCOLORSTATIC too, so tell them from labels.
            WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
                let dc = w as HDC;
                let child = l as HWND;
                if fields.contains(&child) {
                    SetTextColor(dc, rgb(if msg == WM_CTLCOLOREDIT { WHITE } else { TEXT }));
                    SetBkColor(dc, rgb(FIELD));
                    Some(field as LRESULT)
                } else {
                    SetTextColor(dc, labels.get(&(child as usize)).copied().unwrap_or(rgb(TEXT)));
                    SetBkColor(dc, rgb(PAGE));
                    Some(page as LRESULT)
                }
            }
            _ => None,
        }
    })
    .expect("style the window");
    handlers.push(window);
    handlers
}

/// Hides a text field's own edge, which it draws itself because nwg always gives it one: after
/// the field paints, its outermost pixels are painted over in its background colour.
fn hide_edges(field: &nwg::ControlHandle, background: HBRUSH) -> nwg::RawEventHandler {
    use winapi::um::commctrl::DefSubclassProc;
    use winapi::um::winuser::{FrameRect, GetWindowDC, ReleaseDC, WM_NCPAINT};
    if let Some(hwnd) = field.hwnd() {
        dark_scroll_bars(hwnd);
    }
    nwg::bind_raw_event_handler(field, 0x10003, move |hwnd, msg, w, l| unsafe {
        if msg != WM_PAINT && msg != WM_NCPAINT {
            return None;
        }
        let result = DefSubclassProc(hwnd, msg, w, l);
        let mut r: RECT = mem::zeroed();
        GetWindowRect(hwnd, &mut r);
        let whole = RECT {
            left: 0,
            top: 0,
            right: r.right - r.left,
            bottom: r.bottom - r.top,
        };
        let dc = GetWindowDC(hwnd);
        FrameRect(dc, &whole, background);
        ReleaseDC(hwnd, dc);
        Some(result)
    })
    .expect("style the text field")
}

/// Draws `button` as one of SimonSays's: rose for the main action, grey for the others.
pub fn style_button(button: &nwg::Button, primary: bool, font: &nwg::Font) -> nwg::RawEventHandler {
    let look = if primary { &PRIMARY } else { &SECONDARY };
    let font = font.handle;
    nwg::bind_raw_event_handler(&button.handle, 0x10002, move |hwnd, msg, _w, _l| unsafe {
        match msg {
            WM_ERASEBKGND => Some(1),
            WM_PAINT => {
                use winapi::um::wingdi::{
                    BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, DeleteDC, DeleteObject, SRCCOPY,
                };
                let mut ps: PAINTSTRUCT = mem::zeroed();
                let dc = BeginPaint(hwnd, &mut ps);
                // Drawn off-screen and copied in one go, so it never shows half painted.
                let mut r: RECT = mem::zeroed();
                GetClientRect(hwnd, &mut r);
                let buffer = CreateCompatibleDC(dc);
                let bitmap = CreateCompatibleBitmap(dc, r.right, r.bottom);
                let old = SelectObject(buffer, bitmap as HGDIOBJ);
                paint_button(hwnd, buffer, look, font as HGDIOBJ);
                BitBlt(dc, 0, 0, r.right, r.bottom, buffer, 0, 0, SRCCOPY);
                SelectObject(buffer, old);
                DeleteObject(bitmap as HGDIOBJ);
                DeleteDC(buffer);
                EndPaint(hwnd, &ps);
                Some(0)
            }
            _ => None,
        }
    })
    .expect("style the button")
}

/// Paints a button on `dc`, from BeginPaint on the button's own window.
fn paint_button(hwnd: HWND, dc: HDC, look: &ButtonLook, font: HGDIOBJ) {
    // Safety: GDI calls on a live window and its paint DC; every object made here is deleted here.
    unsafe {
        let mut r: RECT = mem::zeroed();
        GetClientRect(hwnd, &mut r);
        let page = CreateSolidBrush(rgb(PAGE));
        FillRect(dc, &r, page);
        winapi::um::wingdi::DeleteObject(page as HGDIOBJ);

        let state = SendMessageW(hwnd, BM_GETSTATE, 0, 0) as u32;
        let fill = if state & BST_PUSHED as u32 != 0 {
            look.pressed
        } else if state & BST_HOT != 0 {
            look.hot
        } else {
            look.idle
        };
        let brush = CreateSolidBrush(rgb(fill));
        let pen = match look.edge {
            Some(c) => CreatePen(PS_SOLID as i32, 1, rgb(c)),
            None => GetStockObject(NULL_PEN as i32) as _,
        };
        let old_brush = SelectObject(dc, brush as HGDIOBJ);
        let old_pen = SelectObject(dc, pen as HGDIOBJ);
        // RoundRect leaves out the right and bottom edges when there's no pen.
        let grow = i32::from(look.edge.is_none());
        RoundRect(dc, r.left, r.top, r.right + grow, r.bottom + grow, ROUND, ROUND);
        SelectObject(dc, old_pen);
        SelectObject(dc, old_brush);
        winapi::um::wingdi::DeleteObject(brush as HGDIOBJ);
        if look.edge.is_some() {
            winapi::um::wingdi::DeleteObject(pen as HGDIOBJ);
        }

        let mut text = [0u16; 64];
        let len = GetWindowTextW(hwnd, text.as_mut_ptr(), text.len() as i32).max(0) as usize;
        let label: Vec<u16> = String::from_utf16_lossy(&text[..len])
            .to_uppercase()
            .encode_utf16()
            .collect();
        let old_font = SelectObject(dc, font);
        SetBkMode(dc, TRANSPARENT as i32);
        SetTextColor(dc, rgb(WHITE));
        DrawTextW(
            dc,
            label.as_ptr(),
            label.len() as i32,
            &mut r,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE,
        );
        SelectObject(dc, old_font);
    }
}

/// `child`'s rectangle in `parent`'s client coordinates.
fn rect_in(parent: HWND, child: HWND) -> RECT {
    // Safety: plain Win32 calls on window handles; a closed window just gives an empty rectangle.
    unsafe {
        let mut r: RECT = mem::zeroed();
        GetWindowRect(child, &mut r);
        MapWindowPoints(std::ptr::null_mut(), parent, &mut r as *mut RECT as *mut POINT, 2);
        r
    }
}

/// Dark scroll bars, as in Explorer's dark mode.
fn dark_scroll_bars(hwnd: HWND) {
    let theme: Vec<u16> = "DarkMode_Explorer\0".encode_utf16().collect();
    // Safety: plain Win32 call with a NUL-terminated string.
    unsafe { winapi::um::uxtheme::SetWindowTheme(hwnd, theme.as_ptr(), std::ptr::null()) };
}

/// A dark title bar, in the page colour where Windows 11 allows it.
fn dark_title_bar(hwnd: HWND) {
    use winapi::um::dwmapi::DwmSetWindowAttribute;
    const DWMWA_USE_IMMERSIVE_DARK_MODE: u32 = 20;
    const DWMWA_CAPTION_COLOR: u32 = 35;
    let dark = TRUE;
    let caption = rgb(PAGE);
    unsafe {
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            &dark as *const _ as *const _,
            mem::size_of_val(&dark) as u32,
        );
        // Older Windows doesn't know this one and leaves the dark title bar as it is.
        DwmSetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_COLOR,
            &caption as *const _ as *const _,
            mem::size_of_val(&caption) as u32,
        );
    }
}
