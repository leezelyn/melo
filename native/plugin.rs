use crate::astrobox::psys_host::{
    device, dialog, interconnect, register, thirdpartyapp, timer, ui_v3 as ui,
};
use crate::exports::astrobox::psys_plugin::{
    event_v3::{self, EventType},
    lifecycle,
};
use crate::{audio, incoming, lyrics, transfer::Sender};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, OnceLock};
use ui::{Element as E, ElementType as T, Event as Ev, FlexDirection as D};
use wit_bindgen::FutureReader;
const PKG: &str = "com.tse.musicplayer";
pub struct Plugin;
struct App {
    roots: Vec<String>,
    busy: bool,
    status: String,
    filename: String,
    file: Option<Arc<[u8]>>,
    title: String,
    start: String,
    end: String,
    lyrics: String,
    lyrics_name: String,
    prepared: Option<Arc<[u8]>>,
    prepared_lyrics: String,
    preview: String,
    profile: String,
    quality: audio::Quality,
    sender: Sender,
    timer: Option<u64>,
    counter: u64,
}
impl Default for App {
    fn default() -> Self {
        Self {
            roots: vec![],
            busy: false,
            status: "选择 MP3，转换后即可传歌".into(),
            filename: String::new(),
            file: None,
            title: String::new(),
            start: "0".into(),
            end: "300".into(),
            lyrics: String::new(),
            lyrics_name: String::new(),
            prepared: None,
            prepared_lyrics: String::new(),
            preview: String::new(),
            profile: "balanced".into(),
            quality: audio::Quality::Original,
            sender: Sender::default(),
            timer: None,
            counter: 0,
        }
    }
}
static STATE: OnceLock<Mutex<App>> = OnceLock::new();
fn state() -> std::sync::MutexGuard<'static, App> {
    STATE
        .get_or_init(|| Mutex::new(App::default()))
        .lock()
        .unwrap_or_else(|e| e.into_inner())
}
fn text(s: &str, size: u32) -> E {
    E::new(T::P, Some(s)).size(size)
}
fn button(s: &str, id: &str, disabled: bool, primary: bool) -> E {
    let e = E::new(T::Button, Some(s))
        .on(Ev::Click, id)
        .padding(12)
        .radius(8)
        .bg(if primary { "#C2EAA1" } else { "#283227" })
        .text_color(if primary { "#182411" } else { "#E8F0E3" });
    if disabled { e.disabled() } else { e }
}
fn field(label: &str, value: &str, id: &str, disabled: bool) -> E {
    let mut input = E::new(T::Input, None)
        .prop("default-value", value)
        .on(Ev::Change, id)
        .width_full();
    if disabled {
        input = input.disabled()
    }
    E::new(T::Div, None)
        .flex()
        .flex_direction(D::Column)
        .gap(8)
        .child(text(label, 13).text_color("#B5C4B0"))
        .child(input)
}
fn view(a: &App) -> E {
    let unfinished = a.sender.unfinished();
    let locked = a.busy || unfinished;
    let mut source = E::new(T::Div, None)
        .flex()
        .flex_direction(D::Column)
        .gap(12)
        .padding(20)
        .radius(14)
        .bg("#191F19")
        .child(text("01  准备歌曲", 18))
        .child(button(
            if a.filename.is_empty() {
                "选择 MP3 文件"
            } else {
                "重新选择 MP3"
            },
            "pick-mp3",
            locked,
            false,
        ));
    if !a.filename.is_empty() {
        source = source.child(text(&a.filename, 13).text_color("#C2EAA1"))
    }
    if a.status == "等待文件选择结果…" {
        source = source.child(text("请在 30 秒内选好文件或取消窗口。", 12).text_color("#9FAC99"));
    }
    source = source.child(field(
        "歌曲名称",
        &a.title,
        "title",
        locked || a.file.is_none(),
    ));
    let clips = E::new(T::Grid, None)
        .grid_template_columns("1fr 1fr")
        .gap(12)
        .child(field(
            "片段起点（秒）",
            &a.start,
            "clip-start",
            locked || a.file.is_none(),
        ))
        .child(field(
            "片段终点（秒）",
            &a.end,
            "clip-end",
            locked || a.file.is_none(),
        ));
    source = source
        .child(clips)
        .child(text("最长 300 秒；短歌会在原曲结尾结束。", 12).text_color("#9FAC99"));
    let mut qualities = E::new(T::Grid, None)
        .grid_template_columns("repeat(3, minmax(0, 1fr))")
        .gap(6);
    for quality in [
        audio::Quality::Original,
        audio::Quality::Mild,
        audio::Quality::Strong,
    ] {
        let selected = a.quality == quality;
        qualities = qualities.child(
            button(
                &format!("{}{}", if selected { "✓ " } else { "" }, quality.label()),
                &format!("quality-{}", quality.id()),
                locked,
                selected,
            )
            .padding(8)
            .size(12)
            .width_full()
            .prop("aria-pressed", if selected { "true" } else { "false" }),
        );
    }
    source = source.child(
        E::new(T::Div, None)
            .flex()
            .flex_direction(D::Column)
            .gap(8)
            .child(text("音质", 13).text_color("#B5C4B0"))
            .child(qualities)
            .child(text(a.quality.description(), 12).text_color("#9FAC99")),
    );
    let mut lrc = E::new(T::Div, None)
        .flex()
        .gap(10)
        .align_center()
        .child(button("添加 LRC 歌词", "pick-lyrics", locked, false));
    if !a.lyrics.is_empty() {
        lrc = lrc.child(text(&a.lyrics_name, 12)).child(button(
            "移除",
            "remove-lyrics",
            locked,
            false,
        ))
    }
    source = source.child(lrc).child(button(
        "转换所选片段",
        "prepare",
        locked || a.file.is_none(),
        false,
    ));
    if let Some(bytes) = &a.prepared {
        source = source.child(
            text(
                &format!(
                    "已准备 · {:.1} 秒 · {} KB · {}{}",
                    bytes.len() as f64 / 12000.0,
                    bytes.len() / 1024,
                    a.quality.label(),
                    if a.prepared_lyrics.is_empty() {
                        ""
                    } else {
                        " · 含歌词"
                    }
                ),
                13,
            )
            .text_color("#C2EAA1"),
        );
        if !unfinished {
            source = source.child(button("试听转换后的片段", "preview", a.busy, false))
        }
    }
    if !a.preview.is_empty() && !unfinished {
        source = source.child(
            E::new(T::Audio, Some(&a.preview))
                .prop("src", &a.preview)
                .width_full(),
        )
    }
    let mut speed = E::new(T::Select, None)
        .prop("value", &a.profile)
        .width_full()
        .on(Ev::Change, "profile");
    for (value, label) in [
        ("balanced", "均衡 · 4 KB"),
        ("stable", "稳定 · 1 KB"),
        ("fast", "快速 · 16 KB"),
    ] {
        speed = speed.child(E::new(T::Option, Some(label)).prop("value", value))
    }
    if locked {
        speed = speed.disabled()
    }
    let phase = match a.sender.phase.as_str() {
        "starting" => "启动传输",
        "sending" => "传输中",
        "saving" => "校验保存",
        "saved" => "已保存",
        "paused" => "已暂停",
        "unconfirmed" => "结果待确认",
        "cancelling" => "等待中断",
        "cancelled" => "已取消",
        "blocked" => "旧记录阻塞",
        _ => "待开始",
    };
    let mut target = E::new(T::Div, None)
        .flex()
        .flex_direction(D::Column)
        .gap(16)
        .padding(22)
        .radius(14)
        .bg("#191F19")
        .child(text("02  传入歌曲", 18))
        .child(text("自动使用 ABox 已连接设备", 13).text_color("#A5B59D"))
        .child(
            E::new(T::Div, None)
                .flex()
                .flex_direction(D::Column)
                .gap(8)
                .child(text("传输速度", 13).text_color("#B5C4B0"))
                .child(speed),
        )
        .child(text(&format!("{} · {}%", phase, a.sender.progress()), 22))
        .child(progress_bar(a.sender.progress()))
        .child(text(&a.status, 13))
        .child(button(
            "开始传歌",
            "send",
            a.busy || unfinished || a.prepared.is_none(),
            true,
        ));
    if matches!(a.sender.phase.as_str(), "paused" | "unconfirmed") {
        target = target.child(button("重试当前任务 / 查询回执", "resume", a.busy, false))
    }
    if unfinished {
        target = target.child(button(
            "取消传输",
            "cancel",
            a.busy || a.sender.phase == "cancelling",
            false,
        ))
    }
    target = target.child(
        text("发送时会自动唤起播放器；请停止手环播放并保持页面打开。", 12).text_color("#9FAC99"),
    );
    let grid = E::new(T::Grid, None)
        .grid_template_columns("repeat(auto-fit, minmax(min(100%, 320px), 1fr))")
        .gap(18)
        .child(source)
        .child(target);
    E::new(T::Div, None)
        .flex()
        .flex_direction(D::Column)
        .gap(14)
        .padding(20)
        .width_full()
        .child(text("环间音乐 · 传歌", 28).text_color("#C2EAA1"))
        .child(text("把喜欢的歌，带到手腕上。", 16))
        .child(grid)
}
fn progress_bar(progress: usize) -> E {
    let progress = progress.min(100);
    let mut track = E::new(T::Div, None)
        .flex()
        .height(8)
        .min_height(8)
        .max_height(8)
        .width_full()
        .flex_shrink(0.0)
        .radius(4)
        .bg("#303A30")
        .prop("role", "progressbar")
        .prop("aria-label", "传歌进度")
        .prop("aria-valuemin", "0")
        .prop("aria-valuemax", "100")
        .prop("aria-valuenow", &progress.to_string());
    if progress > 0 {
        track = track.child(
            E::new(T::Div, None)
                .width(0)
                .min_width(0)
                .height(8)
                .flex_grow(progress as f32)
                .radius(4)
                .bg("#C2EAA1"),
        );
    }
    if progress < 100 {
        track = track.child(
            E::new(T::Div, None)
                .width(0)
                .min_width(0)
                .height(8)
                .flex_grow((100 - progress) as f32),
        );
    }
    track
}
fn paint() {
    let a = state();
    for root in &a.roots {
        ui::render(root, view(&a))
    }
}
fn invalidate(a: &mut App) {
    a.prepared = None;
    a.prepared_lyrics.clear();
    a.preview.clear();
    if !a.sender.unfinished() {
        let serial = a.sender.serial + 1;
        a.sender = Sender::default();
        a.sender.serial = serial;
    }
}
fn input_value(raw: &str) -> String {
    let v = serde_json::from_str::<Value>(raw).unwrap_or(Value::String(raw.into()));
    v.as_str()
        .or_else(|| v.get("value").and_then(Value::as_str))
        .unwrap_or(raw)
        .into()
}
async fn pick(extension: &str) -> dialog::PickResult {
    println!("[huanjian] file picker requested: {extension}");
    let result = dialog::pick_file(
        &dialog::PickConfig {
            read: true,
            copy_to: None,
        },
        &dialog::FilterConfig {
            multiple: false,
            extensions: vec![extension.into()],
            default_directory: String::new(),
            default_file_name: String::new(),
        },
    )
    .await;
    println!(
        "[huanjian] file picker returned: {} bytes",
        result.data.len()
    );
    result
}
fn idle_status(a: &App) -> &'static str {
    if a.prepared.is_some() {
        "片段已准备，可以开始传歌"
    } else if a.file.is_some() {
        "已选歌，请转换所选片段"
    } else {
        "选择 MP3，转换后即可传歌"
    }
}
fn apply_pick(extension: &str, r: dialog::PickResult) -> Result<(), String> {
    if r.name.is_empty() {
        return Ok(());
    }
    if extension == "mp3" {
        if !r.name.to_lowercase().ends_with(".mp3")
            || r.data.is_empty()
            || r.data.len() > 64 * 1024 * 1024
        {
            return Err("请选择有效 MP3，单文件最大 64 MiB".into());
        }
        let mut a = state();
        invalidate(&mut a);
        a.filename = r.name.clone();
        a.title = r
            .name
            .trim_end_matches(".mp3")
            .trim_end_matches(".MP3")
            .chars()
            .filter(|c| *c >= ' ')
            .take(64)
            .collect();
        a.file = Some(Arc::from(r.data));
        a.start = "0".into();
        a.end = "300".into();
        a.status = "已选歌，请转换所选片段".into();
    } else {
        if r.data.len() > 16384 {
            return Err("歌词文件过大".into());
        }
        let raw = String::from_utf8(r.data).map_err(|_| "歌词需为 UTF-8")?;
        lyrics::clip(&raw, 0.0, 3600000.0)?;
        let mut a = state();
        invalidate(&mut a);
        a.lyrics = raw;
        a.lyrics_name = r.name;
    }
    Ok(())
}
fn action_status(id: &str) -> &'static str {
    match id {
        "pick-mp3" | "pick-lyrics" => "等待文件选择结果…",
        "prepare" => "正在解码并转换 MP3…",
        "send" => "正在读取已连接设备并启动传输…",
        "resume" => "正在恢复传输…",
        "cancel" => "正在请求取消传输…",
        _ => "正在处理…",
    }
}
async fn setup_receiver(addr: &str) -> Result<(), String> {
    let apps = thirdpartyapp::get_thirdparty_app_list(addr)
        .await
        .map_err(|_| "读取手环应用失败")?;
    let app = apps
        .iter()
        .find(|a| a.package_name == PKG)
        .ok_or("手环未安装环间音乐")?;
    register::register_interconnect_recv(addr, PKG)
        .await
        .map_err(|_| "注册歌曲回执失败，请检查插件接收消息权限后重试")?;
    thirdpartyapp::launch_qa(addr, app, "pages/index")
        .await
        .map_err(|_| "无法唤起手环播放器，请重试")?;
    Ok(())
}
async fn dispatch() {
    let (old, serial, due, packet, addr) = {
        let mut a = state();
        let s = &a.sender;
        let data = (s.serial, s.due.clone(), s.pending.clone(), s.addr.clone());
        (a.timer.take(), data.0, data.1, data.2, data.3)
    };
    if let Some(id) = old {
        timer::clear_timer(id).await
    }
    let Some((ms, kind)) = due else { return };
    let payload = json!({"serial":serial,"kind":kind}).to_string();
    let id = timer::set_timeout(ms, &payload).await;
    let still_current = {
        let mut a = state();
        if a.sender.serial == serial {
            a.timer = Some(id);
            true
        } else {
            false
        }
    };
    if !still_current {
        timer::clear_timer(id).await;
        return;
    }
    if kind == "timeout" {
        if let Some(packet) = packet {
            if interconnect::send_qaic_message(&addr, PKG, &packet.to_string())
                .await
                .is_err()
            {
                let mut a = state();
                if a.sender.serial == serial {
                    a.sender.status = "消息发送失败，等待重试".into();
                    a.status = a.sender.status.clone()
                }
            }
        }
    }
}
async fn action(id: &str) -> Result<(), String> {
    match id {
        "pick-mp3" => apply_pick("mp3", pick("mp3").await)?,
        "pick-lyrics" => apply_pick("lrc", pick("lrc").await)?,
        "remove-lyrics" => {
            let mut a = state();
            invalidate(&mut a);
            a.lyrics.clear();
            a.lyrics_name.clear()
        }
        "prepare" => {
            let (data, start, end, lrc, quality) = {
                let a = state();
                (
                    a.file.clone().ok_or("请先选择 MP3")?,
                    a.start.parse::<f64>().map_err(|_| "起点无效")?,
                    a.end.parse::<f64>().map_err(|_| "终点无效")?,
                    a.lyrics.clone(),
                    a.quality,
                )
            };
            let bytes = audio::decode_clip_with_quality(data, start, end, quality)?;
            let text = lyrics::clip(&lrc, start, start + bytes.len() as f64 / 12000.0)?;
            let mut a = state();
            a.prepared = Some(Arc::from(bytes));
            a.prepared_lyrics = text;
            a.preview.clear();
            a.status = "转换完成，可以开始传歌".into()
        }
        "preview" => {
            let bytes = state().prepared.clone().ok_or("请先转换歌曲")?;
            state().preview = audio::wav_data_uri(&bytes)
        }
        "send" => {
            let (title, bytes, lrc, profile, peak) = {
                let a = state();
                (
                    a.title.trim().to_string(),
                    a.prepared.clone().ok_or("请先转换歌曲")?,
                    a.prepared_lyrics.clone(),
                    a.profile.clone(),
                    a.quality.peak(),
                )
            };
            if title.is_empty()
                || title.encode_utf16().count() > 64
                || title.chars().any(|c| c < ' ')
            {
                return Err("歌曲名称需为 1–64 字符".into());
            }
            let device = device::get_connected_device_list()
                .await
                .into_iter()
                .next()
                .ok_or("当前设备连接暂不可用，请重试")?;
            setup_receiver(&device.addr).await?;
            let id = {
                let mut a = state();
                a.counter += 1;
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map_err(|_| "系统时间无效")?
                    .as_nanos() as u64;
                format!("{now:016x}{:016x}", a.counter)
            };
            {
                let mut a = state();
                a.preview.clear();
                a.sender
                    .start(id, device.addr, title, bytes.to_vec(), lrc, &profile, peak)?;
                a.status = a.sender.status.clone()
            }
            dispatch().await;
        }
        id if id.starts_with("quality-") => {
            let quality = audio::Quality::parse(&id[8..]).ok_or("音质选项无效")?;
            let mut a = state();
            if a.quality != quality {
                a.quality = quality;
                invalidate(&mut a);
                a.status = "音质已切换，请重新转换所选片段".into();
            } else {
                a.status = format!("当前音质：{}", quality.label());
            }
        }
        "resume" => {
            {
                let mut a = state();
                a.sender.resume()?;
                a.status = a.sender.status.clone()
            }
            dispatch().await
        }
        "cancel" => {
            {
                let mut a = state();
                a.sender.cancel();
                a.status = a.sender.status.clone()
            }
            dispatch().await
        }
        _ => {}
    }
    Ok(())
}
impl lifecycle::Guest for Plugin {
    fn on_load() {
        println!(
            "[huanjian {}] loaded; API3/WASI2; synchronous V3 callbacks",
            env!("CARGO_PKG_VERSION")
        );
    }
}
impl event_v3::Guest for Plugin {
    fn on_ui_render(id: String) -> FutureReader<()> {
        println!("[huanjian] render callback entered");
        wit_bindgen::block_on(async {});
        let (w, r) = crate::wit_future::new(|| ());
        {
            let mut a = state();
            if !a.roots.contains(&id) {
                a.roots.push(id)
            }
        }
        // First paint is independent of every asynchronous host call.
        paint();
        println!("[huanjian] first UI emitted");
        wit_bindgen::spawn(async move {
            let _ = w.write(()).await;
        });
        r
    }
    fn on_card_render(id: String) -> FutureReader<()> {
        wit_bindgen::block_on(async {});
        let (w, r) = crate::wit_future::new(|| ());
        ui::render_to_text_card(&id, &state().status);
        wit_bindgen::spawn(async move {
            let _ = w.write(()).await;
        });
        r
    }
    fn on_ui_event_v3(id: String, event: Ev, payload: String) -> FutureReader<String> {
        let (w, r) = crate::wit_future::new(String::new);
        // V3's host calls once and drains the returned future without a resident
        // executor. Finish host waits in this synchronous export's scope; work
        // spawned after returning the reader can remain pending indefinitely.
        wit_bindgen::block_on(async move {
            if matches!(event, Ev::Change | Ev::Input) {
                let mut a = state();
                if !a.busy && !a.sender.unfinished() {
                    let value = input_value(&payload);
                    match id.as_str() {
                        "title" => a.title = value,
                        "clip-start" => {
                            a.start = value;
                            invalidate(&mut a)
                        }
                        "clip-end" => {
                            a.end = value;
                            invalidate(&mut a)
                        }
                        "profile" => a.profile = value,
                        _ => {}
                    }
                }
                drop(a);
                if id == "profile" {
                    paint()
                }
            } else if event == Ev::Click {
                let allowed = {
                    let mut a = state();
                    if a.busy
                        || a.sender.unfinished() && !matches!(id.as_str(), "resume" | "cancel")
                    {
                        false
                    } else {
                        a.busy = true;
                        a.status = action_status(&id).into();
                        true
                    }
                };
                if allowed {
                    paint();
                    let result = action(&id).await;
                    {
                        let mut a = state();
                        a.busy = false;
                        if let Err(e) = result {
                            a.status = e.clone();
                            if a.sender.unfinished() {
                                a.sender.status = e
                            }
                        } else if !a.busy && a.status == action_status(&id) {
                            a.status = idle_status(&a).into();
                        }
                    }
                    paint()
                }
            }
        });
        wit_bindgen::spawn(async move {
            let _ = w.write(String::new()).await;
        });
        r
    }
    fn on_event(kind: EventType, payload: String) -> FutureReader<String> {
        let (w, r) = crate::wit_future::new(String::new);
        wit_bindgen::block_on(async move {
            let mut changed = false;
            if kind == EventType::InterconnectMessage {
                let mut a = state();
                if let Some(v) = incoming::decode(&payload, &a.sender.addr, PKG) {
                    let before = a.sender.serial;
                    a.sender.receive(v);
                    changed = a.sender.serial != before;
                }
                println!(
                    "[huanjian] interconnect event: bytes={}, accepted={changed}",
                    payload.len()
                );
            } else if kind == EventType::Timer {
                if let Ok(v) = serde_json::from_str::<Value>(&payload) {
                    if let Some(id) = v["timerId"].as_u64() {
                        let mut a = state();
                        // Clearing a firing timer aborts this host callback;
                        // the host removes a one-shot timer after dispatch.
                        if a.timer == Some(id) {
                            a.timer = None;
                        }
                    }
                    let t = if let Some(p) = v["payload"].as_str() {
                        serde_json::from_str::<Value>(p).unwrap_or(Value::Null)
                    } else {
                        v
                    };
                    if let (Some(serial), Some(k)) = (t["serial"].as_u64(), t["kind"].as_str()) {
                        let mut a = state();
                        let before = a.sender.serial;
                        a.sender.timeout(serial, k);
                        changed = a.sender.serial != before
                    }
                }
            }
            if changed {
                {
                    let mut a = state();
                    a.status = a.sender.status.clone()
                }
                dispatch().await;
                paint()
            }
        });
        wit_bindgen::spawn(async move {
            let _ = w.write(String::new()).await;
        });
        r
    }
}
