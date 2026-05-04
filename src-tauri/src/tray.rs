use tauri::AppHandle;

pub enum TrayState {
    Normal,
    Recording,
    Thinking,
}

pub fn set_icon(app: &AppHandle, state: TrayState) {
    let bytes: &[u8] = match state {
        TrayState::Normal => include_bytes!("../icons/32x32.png"),
        TrayState::Recording => include_bytes!("../icons/tray-recording.png"),
        TrayState::Thinking => include_bytes!("../icons/tray-thinking.png"),
    };

    let image = match tauri::image::Image::from_bytes(bytes) {
        Ok(img) => img,
        Err(e) => {
            crate::app_log!("[tray] Ikon-fejl: {}", e);
            return;
        }
    };

    if let Some(tray) = app.tray_by_id("main") {
        let _ = tray.set_icon(Some(image));
        let _ = tray.set_icon_as_template(false);
    }
}
