use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Edge { #[default] Right, Left, Top, Bottom }
impl Edge {
    pub fn index(self) -> usize { match self { Self::Right => 0, Self::Left => 1, Self::Top => 2, Self::Bottom => 3 } }
    pub fn vertical(self) -> bool { matches!(self, Self::Right | Self::Left) }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Bounds { pub x: i32, pub y: i32, pub width: u32, pub height: u32 }
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Display { pub id: String, pub name: String, pub bounds: Bounds, pub work: Bounds, pub scale: f64, pub primary: bool }

pub fn frame(area: Bounds, edge: Edge, scale: f64) -> Bounds {
    let depth = (430.0 * scale).round().max(1.0) as u32;
    let (width, height) = if edge.vertical() { (depth.min(area.width), area.height) } else { (area.width, depth.min(area.height)) };
    Bounds { x: area.x + if edge == Edge::Right { (area.width - width) as i32 } else { 0 },
        y: area.y + if edge == Edge::Bottom { (area.height - height) as i32 } else { 0 }, width, height }
}

pub fn displays(app: &AppHandle) -> Vec<Display> {
    let Ok(monitors) = app.available_monitors() else { return vec![] };
    let primary = app.primary_monitor().ok().flatten();
    monitors.into_iter().map(|m| {
        let name = m.name().cloned().unwrap_or_default();
        let bounds = Bounds { x: m.position().x, y: m.position().y, width: m.size().width, height: m.size().height };
        let mut d = Display { id: name.clone(), name, bounds, work: bounds, scale: m.scale_factor(), primary: primary.as_ref().map(|p| p.position() == m.position()).unwrap_or(false) };
        #[cfg(windows)]
        unsafe {
            use windows::Win32::Graphics::Gdi::*;
            use windows::Win32::Foundation::POINT;
            use windows::Win32::UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME;
            let mon = MonitorFromPoint(POINT { x: bounds.x + 1, y: bounds.y + 1 }, MONITOR_DEFAULTTONEAREST);
            let mut info = MONITORINFOEXW::default(); info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
            if GetMonitorInfoW(mon, &mut info as *mut _ as *mut MONITORINFO).as_bool() {
                let r = info.monitorInfo.rcWork;
                d.work = Bounds { x: r.left, y: r.top, width: (r.right-r.left).max(1) as u32, height: (r.bottom-r.top).max(1) as u32 };
                let mut dev = DISPLAY_DEVICEW::default(); dev.cb = std::mem::size_of::<DISPLAY_DEVICEW>() as u32;
                if EnumDisplayDevicesW(windows::core::PCWSTR(info.szDevice.as_ptr()), 0, &mut dev, EDD_GET_DEVICE_INTERFACE_NAME).as_bool() {
                    let text = |v: &[u16]| String::from_utf16_lossy(&v[..v.iter().position(|c| *c==0).unwrap_or(v.len())]);
                    let id = text(&dev.DeviceID); if !id.is_empty() { d.id = id; }
                    d.name = format!("{} · {}", text(&dev.DeviceString), d.name);
                }
            }
        }
        d
    }).collect()
}
pub fn selected(app: &AppHandle, cfg: &crate::config::Config) -> Option<Display> {
    let ds = displays(app);
    ds.iter().find(|d| d.id == cfg.monitor_id).or_else(|| ds.iter().find(|d| d.primary)).or(ds.first()).cloned()
}
pub fn apply(app: &AppHandle) {
    let cfg = app.state::<crate::AppState>().cfg.lock().unwrap().clone();
    let (Some(d), Some(w)) = (selected(app, &cfg), app.get_webview_window("notch")) else { return };
    let area = if cfg.avoid_taskbar { d.work } else { d.bounds };
    let f = frame(area, cfg.edge, d.scale * cfg.scale);
    // Place on the destination monitor before sizing, so WebView2 adopts its DPI.
    let _ = w.set_position(tauri::PhysicalPosition::new(f.x, f.y));
    let _ = w.set_size(tauri::PhysicalSize::new(f.width, f.height));
    let _ = w.set_always_on_top(cfg.always_on_top);
    crate::noactivate(app);
    use tauri::Emitter;
    let _ = app.emit("settings", &cfg);
}

pub fn fullscreen(app: &AppHandle, cfg: &crate::config::Config) -> bool {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::{Foundation::RECT, UI::WindowsAndMessaging::*};
        let Some(d) = selected(app, cfg) else { return false };
        let fg = GetForegroundWindow();
        if fg.is_invalid() || fg == GetDesktopWindow() || fg == GetShellWindow() { return false; }
        let mut pid = 0; GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == std::process::id() { return false; }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() { return false; }
        let b = d.bounds;
        return r.left <= b.x && r.top <= b.y && r.right >= b.x+b.width as i32 && r.bottom >= b.y+b.height as i32;
    }
    #[cfg(not(windows))] { let _ = (app, cfg); false }
}

/// Restrict native hit testing to the rendered shapes. Windows owns the final region.
pub fn set_region(app: &AppHandle, rects: &[[f64; 5]]) {
    #[cfg(windows)]
    unsafe {
        use windows::Win32::Graphics::Gdi::*;
        let Some(w) = app.get_webview_window("notch") else { return };
        let Ok(h) = w.hwnd() else { return };
        let hwnd = windows::Win32::Foundation::HWND(h.0 as *mut _);
        let region = CreateRectRgn(0,0,0,0);
        if region.is_invalid() { return; }
        for r in rects.iter().take(12) {
            if r.iter().any(|v| !v.is_finite() || v.abs() > 100_000.0) || r[2] <= 0.0 || r[3] <= 0.0 { continue; }
            let (x,y,right,bottom)=(r[0].floor() as i32,r[1].floor() as i32,(r[0]+r[2]).ceil() as i32+1,(r[1]+r[3]).ceil() as i32+1);
            let part = if r[4] <= -5.0 {
                let region=CreateRectRgn(x,y,right,bottom);
                let cx=if r[4]==-5.0 || r[4]==-6.0 {x} else {right};
                let cy=if r[4]==-5.0 || r[4]==-7.0 {y} else {bottom};
                let circle=CreateEllipticRgn(cx-(right-x),cy-(bottom-y),cx+(right-x),cy+(bottom-y));
                CombineRgn(region,region,circle,RGN_DIFF); let _=DeleteObject(circle); region
            } else if r[4] < 0.0 {
                use windows::Win32::Foundation::POINT;
                let pts=match r[4] as i32 {
                    -1=>[(x,y),(right,(y+bottom)/2),(x,bottom)],
                    -2=>[(right,y),(x,(y+bottom)/2),(right,bottom)],
                    -3=>[(x,bottom),((x+right)/2,y),(right,bottom)],
                    _=>[(x,y),((x+right)/2,bottom),(right,y)],
                }.map(|(x,y)|POINT{x,y});
                CreatePolygonRgn(&pts,WINDING)
            } else { CreateRoundRectRgn(x,y,right,bottom,r[4] as i32,r[4] as i32) };
            CombineRgn(region, region, part, RGN_OR); let _ = DeleteObject(part);
        }
        if SetWindowRgn(hwnd, region, true) == 0 { let _ = DeleteObject(region); }
    }
    #[cfg(not(windows))] let _ = (app, rects);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_edge_stays_on_its_monitor_at_mixed_dpi() {
        for area in [Bounds {x:-2560,y:-400,width:2560,height:1400}, Bounds{x:0,y:0,width:320,height:240}] {
            for edge in [Edge::Right,Edge::Left,Edge::Top,Edge::Bottom] {
                for dpi in [1.0,1.25,1.5,2.0] { for size in [0.75,1.0,1.5] {
                    let f=frame(area,edge,dpi*size);
                    assert!(f.x>=area.x && f.y>=area.y);
                    assert!(f.x+f.width as i32<=area.x+area.width as i32);
                    assert!(f.y+f.height as i32<=area.y+area.height as i32);
                    match edge { Edge::Right=>assert_eq!(f.x+f.width as i32,area.x+area.width as i32), Edge::Left=>assert_eq!(f.x,area.x), Edge::Top=>assert_eq!(f.y,area.y), Edge::Bottom=>assert_eq!(f.y+f.height as i32,area.y+area.height as i32) }
                }}
            }
        }
    }
}
