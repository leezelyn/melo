// Exact ABox V3 engine version. Local UI/file adapters; no AstroBox or Bluetooth.
use anyhow::{Result, anyhow, ensure};
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use wasmtime::component::{
    Component, FutureConsumer, FutureReader, Instance, Linker, Resource, ResourceAny,
    ResourceTable, ResourceType, Source, Val,
};
use wasmtime::{Config, Engine, Store, StoreContextMut};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
#[derive(Clone, Debug, Default)]
struct Element {
    kind: String,
    text: String,
    children: Vec<Element>,
    events: Vec<String>,
    disabled: bool,
    attrs: std::collections::BTreeMap<String, String>,
}
fn capture(ctx: &Ctx, name: &str) -> Result<()> {
    let Ok(directory) = std::env::var("UI_PREVIEW_DIR") else { return Ok(()) };
    fn tree(e: &Element) -> serde_json::Value {
        serde_json::json!({"kind":e.kind,"text":e.text,"disabled":e.disabled,"attrs":e.attrs,
            "children":e.children.iter().map(tree).collect::<Vec<_>>()})
    }
    std::fs::create_dir_all(&directory)?;
    std::fs::write(std::path::Path::new(&directory).join(format!("{name}.json")), tree(ctx.renders.last().unwrap()).to_string())?;
    Ok(())
}
#[derive(
    wasmtime::component::ComponentType, wasmtime::component::Lift, wasmtime::component::Lower,
)]
#[component(record)]
struct PickResult {
    name: String,
    data: Vec<u8>,
}
#[derive(
    wasmtime::component::ComponentType, wasmtime::component::Lift, wasmtime::component::Lower,
)]
#[component(record)]
struct DeviceInfo {
    name: String,
    addr: String,
}
#[derive(
    wasmtime::component::ComponentType, wasmtime::component::Lift, wasmtime::component::Lower,
)]
#[component(record)]
struct AppInfo {
    #[component(name = "package-name")]
    package_name: String,
    fingerprint: Vec<u32>,
    #[component(name = "version-code")]
    version_code: u32,
    #[component(name = "can-remove")]
    can_remove: bool,
    #[component(name = "app-name")]
    app_name: String,
}
struct Ctx {
    wasi: WasiCtx,
    table: ResourceTable,
    instance: Option<Instance>,
    renders: Vec<Element>,
    picks: usize,
    calls: Vec<&'static str>,
    sent: Vec<String>,
    timers: std::collections::BTreeMap<u64, String>,
    timer_serial: u64,
    firing: Option<u64>,
    permissions: Vec<String>,
}
impl WasiView for Ctx {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}
fn string(v: &Val) -> String {
    match v {
        Val::String(s) | Val::Enum(s) => s.clone(),
        Val::Option(Some(v)) => string(v),
        Val::U32(v) => v.to_string(),
        Val::Float32(v) => v.to_string(),
        _ => String::new(),
    }
}
// V3 checks operation names individually; interconnect does not grant receive
// registration. Read the actual artifact manifest instead of granting all calls.
fn permitted(ctx: &Ctx, operation: &str) -> bool {
    let declared = ctx.permissions.iter().any(|name| name == operation);
    if !declared {
        eprintln!("V3 host: permission '{operation}' not declared by plugin");
    }
    declared
}
fn resource(v: &Val, store: &mut StoreContextMut<'_, Ctx>) -> Result<Resource<Element>> {
    if let Val::Resource(r) = v {
        r.try_into_resource(store)
    } else {
        Err(anyhow!("expected element"))
    }
}
fn value(e: Element, mut store: StoreContextMut<'_, Ctx>) -> Result<Val> {
    let r = store.data_mut().table.push(e)?;
    Ok(Val::Resource(ResourceAny::try_from_resource(r, store)?))
}
fn ready<T: wasmtime::component::Lower + wasmtime::component::Lift + Send + Sync + 'static>(
    mut store: StoreContextMut<'_, Ctx>,
    value: T,
) -> Val {
    let instance = store.data().instance.unwrap();
    FutureReader::new(instance, &mut store, async move {
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        Ok::<_, anyhow::Error>(value)
    })
    .into_val()
}
// Match ABox V3: call once, pipe result, yield; no permanent run_concurrent.
struct Drain;
impl<D> FutureConsumer<D> for Drain {
    type Item = String;
    fn poll_consume(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, String>,
        _: bool,
    ) -> Poll<Result<()>> {
        let mut v = None;
        source.read(store, &mut v)?;
        Poll::Ready(Ok(()))
    }
}
struct DrainUnit;
impl<D> FutureConsumer<D> for DrainUnit {
    type Item = ();
    fn poll_consume(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        store: StoreContextMut<D>,
        mut source: Source<'_, ()>,
        _: bool,
    ) -> Poll<Result<()>> {
        let mut v = None;
        source.read(store, &mut v)?;
        Poll::Ready(Ok(()))
    }
}
async fn legacy_call(
    instance: Instance,
    store: &mut Store<Ctx>,
    func: wasmtime::component::Func,
    params: &[Val],
) -> Result<()> {
    let mut result = [Val::Bool(false)];
    tokio::time::timeout(
        std::time::Duration::from_secs(4),
        func.call_async(&mut *store, params, &mut result),
    )
    .await
    .map_err(|_| anyhow!("V3 callback did not finish"))??;
    func.post_return_async(&mut *store).await?;
    let f = FutureReader::<String>::from_val(&mut *store, instance, &result[0])?;
    f.pipe(&mut *store, Drain);
    tokio::task::yield_now().await;
    Ok(())
}
async fn click(instance: Instance, store: &mut Store<Ctx>, id: &str) -> Result<()> {
    let event = instance
        .get_export_index(&mut *store, None, "astrobox:psys-plugin/event-v3")
        .unwrap();
    let idx = instance
        .get_export_index(&mut *store, Some(&event), "on-ui-event-v3")
        .unwrap();
    let f = instance.get_func(&mut *store, idx).unwrap();
    legacy_call(
        instance,
        store,
        f,
        &[
            Val::String(id.into()),
            Val::Enum("CLICK".into()),
            Val::String(String::new()),
        ],
    )
    .await?;
    Ok(())
}
async fn incoming(
    instance: Instance,
    store: &mut Store<Ctx>,
    callback: wasmtime::component::Func,
    message: &serde_json::Value,
    format: &str,
) -> Result<()> {
    let payload = if format == "raw" {
        message.to_string()
    } else {
        serde_json::json!({"addr":"fixture-device","pkgName":"com.tse.musicplayer",
            "payloadText":message.to_string(),
            "payloadHex":message.to_string().bytes().map(|b| format!("{b:02x}")).collect::<String>()}).to_string()
    };
    legacy_call(
        instance,
        store,
        callback,
        &[
            Val::Enum("interconnect-message".into()),
            Val::String(payload),
        ],
    )
    .await
}
#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let path = std::env::args().nth(1).unwrap_or("dist/plugin.wasm".into());
    let incoming_format = std::env::args().nth(3).unwrap_or("payloadText".into());
    let recovery = std::env::args().nth(4).unwrap_or("normal".into());
    let manifest_path = std::env::args()
        .nth(2)
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::Path::new(&path).with_file_name("manifest.json"));
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(manifest_path)?)?;
    let permissions: Vec<String> = serde_json::from_value(manifest["permissions"].clone())?;
    let mut config = Config::new();
    config
        .async_support(true)
        .wasm_component_model(true)
        .wasm_component_model_async(true)
        .cranelift_opt_level(wasmtime::OptLevel::None);
    let engine = Engine::new(&config)?;
    let component = Component::from_file(&engine, path)?;
    let mut linker = Linker::<Ctx>::new(&engine);
    wasmtime_wasi::p2::add_to_linker_async(&mut linker)?;
    {
        let mut ui = linker.instance("astrobox:psys-host/ui-v3")?;
        ui.resource(
            "element",
            ResourceType::host::<Element>(),
            |mut store, rep| {
                store
                    .data_mut()
                    .table
                    .delete(Resource::<Element>::new_own(rep))?;
                Ok(())
            },
        )?;
        ui.func_new("[constructor]element", |store, p, r| {
            r[0] = value(
                Element {
                    kind: string(&p[0]),
                    text: string(&p[1]),
                    ..Default::default()
                },
                store,
            )?;
            Ok(())
        })?;
        let wit = std::fs::read_to_string("wit/deps/astrobox-psys-host.wit")?;
        let body = wit
            .split("interface ui-v3 {")
            .nth(1)
            .unwrap()
            .split("enum event {")
            .next()
            .unwrap();
        for line in body.lines() {
            let line = line.trim();
            let Some((method, rest)) = line.split_once(':') else {
                continue;
            };
            if !rest.trim_start().starts_with("func(") {
                continue;
            }
            let name = method.to_string();
            ui.func_new(
                &format!("[method]element.{method}"),
                move |mut store, p, r| {
                    let own = resource(&p[0], &mut store)?;
                    let mut e = store.data().table.get(&own)?.clone();
                    match name.as_str() {
                        "child" => {
                            let child = resource(&p[1], &mut store)?;
                            e.children.push(store.data_mut().table.delete(child)?)
                        }
                        "on" => e.events.push(string(&p[2])),
                        "disabled" => e.disabled = true,
                        "content" => e.text = string(&p[1]),
                        "prop" => { e.attrs.insert(format!("prop:{}", string(&p[1])), string(&p[2])); }
                        _ => { e.attrs.insert(name.clone(), p.get(1).map(string).unwrap_or_default()); }
                    }
                    r[0] = value(e, store)?;
                    Ok(())
                },
            )?;
        }
        ui.func_new("render", |mut store, p, _| {
            let r = resource(&p[1], &mut store)?;
            let tree = store.data_mut().table.delete(r)?;
            store.data_mut().renders.push(tree);
            Ok(())
        })?;
        ui.func_new("render-to-text-card", |_, _, _| Ok(()))?;
        ui.func_new("get-render-size", |_, _, _| {
            Err(anyhow!("first paint must not await render-size"))
        })?;
    }
    {
        // A large but valid MP3: 8 MiB ID3 padding followed by owned audio.
        // Exercises returning a large list<u8>, without checking in a large file.
        let tone = std::fs::read("tests/fixtures/tone.mp3")?;
        let offset = if tone.starts_with(b"ID3") {
            10 + tone[6..10]
                .iter()
                .fold(0usize, |n, b| (n << 7) | *b as usize)
        } else {
            0
        };
        let padding = 8 * 1024 * 1024usize;
        let mut fixture = b"ID3\x03\0\0".to_vec();
        for shift in [21, 14, 7, 0] {
            fixture.push(((padding >> shift) & 127) as u8);
        }
        fixture.resize(padding + 10, 0);
        fixture.extend_from_slice(&tone[offset..]);
        linker.instance("astrobox:psys-host/dialog")?.func_new(
            "pick-file",
            move |mut store, p, r| {
                let _ = p;
                store.data_mut().picks += 1;
                let pick = store.data().picks;
                let instance = store.data().instance.unwrap();
                let (name, data) = match pick {
                    1 | 4 => ("", vec![]), // native dialog cancellation
                    2 => ("empty.mp3", vec![]),
                    3 => ("tone.mp3", fixture.clone()),
                    5 => ("tone.lrc", b"[00:00.00]tone".to_vec()),
                    _ => return Err(anyhow!("unexpected picker request")),
                };
                r[0] = FutureReader::new(instance, &mut store, async move {
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Ok::<_, anyhow::Error>(PickResult {
                        name: name.into(),
                        data,
                    })
                })
                .into_val();
                Ok(())
            },
        )?;
    }
    linker.instance("astrobox:psys-host/device")?.func_new(
        "get-connected-device-list",
        |mut s, _, r| {
            s.data_mut().calls.push("devices");
            if !permitted(s.data(), "device") {
                r[0] = ready(s, Vec::<DeviceInfo>::new());
                return Ok(());
            }
            r[0] = ready(
                s,
                vec![DeviceInfo {
                    name: "fixture".into(),
                    addr: "fixture-device".into(),
                }],
            );
            Ok(())
        },
    )?;
    {
        let mut apps = linker.instance("astrobox:psys-host/thirdpartyapp")?;
        apps.func_new("get-thirdparty-app-list", |mut s, _, r| {
            s.data_mut().calls.push("apps");
            if !permitted(s.data(), "thirdpartyapp") {
                r[0] = ready(s, Err::<Vec<AppInfo>, ()>(()));
                return Ok(());
            }
            r[0] = ready(
                s,
                Ok::<_, ()>(vec![AppInfo {
                    package_name: "com.tse.musicplayer".into(),
                    fingerprint: vec![1],
                    version_code: 65,
                    can_remove: true,
                    app_name: "环间音乐".into(),
                }]),
            );
            Ok(())
        })?;
        apps.func_new("launch-qa", |mut s, _, r| {
            s.data_mut().calls.push("launch");
            let result = if permitted(s.data(), "thirdpartyapp") {
                Ok(())
            } else {
                Err(())
            };
            r[0] = ready(s, result);
            Ok(())
        })?;
    }
    linker.instance("astrobox:psys-host/register")?.func_new(
        "register-interconnect-recv",
        |mut s, _, r| {
            s.data_mut().calls.push("register");
            let result = if permitted(s.data(), "register_interconnect_recv") {
                Ok(())
            } else {
                Err(())
            };
            r[0] = ready(s, result);
            Ok(())
        },
    )?;
    {
        let mut timers = linker.instance("astrobox:psys-host/timer")?;
        timers.func_new("set-interval", |_, _, _| {
            Err(anyhow!("V3 callback must not need a heartbeat"))
        })?;
        timers.func_new("set-timeout", |mut s, p, r| {
            s.data_mut().timer_serial += 1;
            let id = s.data().timer_serial;
            s.data_mut().timers.insert(id, string(&p[1]));
            r[0] = ready(s, id);
            Ok(())
        })?;
        timers.func_new("clear-timer", |mut s, p, r| {
            let Val::U64(id) = p[0] else {
                return Err(anyhow!("timer id missing"));
            };
            ensure!(
                s.data().firing != Some(id),
                "clearing the firing timer aborts the host task driving this callback"
            );
            s.data_mut().timers.remove(&id);
            r[0] = ready(s, ());
            Ok(())
        })?;
    }
    linker
        .instance("astrobox:psys-host/interconnect")?
        .func_new("send-qaic-message", |mut s, p, r| {
            s.data_mut().calls.push("send");
            if !permitted(s.data(), "interconnect") {
                r[0] = ready(s, Err::<(), ()>(()));
                return Ok(());
            }
            s.data_mut().sent.push(string(&p[2]));
            r[0] = ready(s, Ok::<(), ()>(()));
            Ok(())
        })?;
    let mut store = Store::new(
        &engine,
        Ctx {
            wasi: WasiCtxBuilder::new().inherit_stdio().build(),
            table: ResourceTable::new(),
            instance: None,
            renders: vec![],
            picks: 0,
            calls: vec![],
            sent: vec![],
            timers: Default::default(),
            timer_serial: 0,
            firing: None,
            permissions,
        },
    );
    let instance = linker.instantiate_async(&mut store, &component).await?;
    store.data_mut().instance = Some(instance);
    let lifecycle = instance
        .get_export_index(&mut store, None, "astrobox:psys-plugin/lifecycle")
        .unwrap();
    let load = instance
        .get_export_index(&mut store, Some(&lifecycle), "on-load")
        .unwrap();
    let load = instance.get_typed_func::<(), ()>(&mut store, load)?;
    load.call_async(&mut store, ()).await?;
    load.post_return_async(&mut store).await?;
    let event = instance
        .get_export_index(&mut store, None, "astrobox:psys-plugin/event-v3")
        .unwrap();
    let render_idx = instance
        .get_export_index(&mut store, Some(&event), "on-ui-render")
        .unwrap();
    let render =
        instance.get_typed_func::<(String,), (FutureReader<()>,)>(&mut store, render_idx)?;
    let (future,) = render.call_async(&mut store, ("features".into(),)).await?;
    render.post_return_async(&mut store).await?;
    future.pipe(&mut store, DrainUnit);
    tokio::task::yield_now().await;
    ensure!(
        !store.data().renders.is_empty(),
        "ui.render was never called"
    );
    ensure!(
        store.data().renders.last().unwrap().kind == "DIV",
        "root element is not a DIV"
    );
    fn contains(e: &Element, id: &str) -> bool {
        e.events.iter().any(|x| x == id) || e.children.iter().any(|e| contains(e, id))
    }
    ensure!(
        contains(store.data().renders.last().unwrap(), "pick-mp3"),
        "file picker button missing"
    );
    ensure!(
        !contains(store.data().renders.last().unwrap(), "connect"),
        "connection step returned"
    );
    fn text_contains(e: &Element, s: &str) -> bool {
        e.text.contains(s) || e.children.iter().any(|e| text_contains(e, s))
    }
    fn enabled(e: &Element, id: &str) -> bool {
        (e.events.iter().any(|s| s == id) && !e.disabled)
            || e.children.iter().any(|e| enabled(e, id))
    }
    fn thin_progress(e: &Element) -> bool {
        (e.attrs.get("prop:role").map(String::as_str) == Some("progressbar")
            && e.attrs.get("height").map(String::as_str) == Some("8")
            && e.attrs.contains_key("width-full")) || e.children.iter().any(thin_progress)
    }
    ensure!(thin_progress(store.data().renders.last().unwrap()), "progress must be an explicit 8px track");
    for id in ["quality-original", "quality-mild", "quality-strong"] {
        ensure!(contains(store.data().renders.last().unwrap(), id), "quality choice missing: {id}");
    }
    capture(store.data(), "plugin-initial")?;
    click(instance, &mut store, "pick-mp3").await?;
    ensure!(
        enabled(store.data().renders.last().unwrap(), "pick-mp3"),
        "cancelling native picker leaves controls locked"
    );
    click(instance, &mut store, "pick-mp3").await?;
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "有效 MP3"),
        "invalid MP3 did not show error"
    );
    ensure!(
        enabled(store.data().renders.last().unwrap(), "pick-mp3"),
        "invalid file leaves controls locked"
    );
    click(instance, &mut store, "pick-mp3").await?;
    ensure!(store.data().picks == 3, "retry did not open picker");
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "tone.mp3"),
        "selected MP3 remains pending after legacy V3 dispatch"
    );
    ensure!(
        enabled(store.data().renders.last().unwrap(), "prepare"),
        "file import leaves controls locked"
    );
    click(instance, &mut store, "prepare").await?;
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "已准备 ·"),
        "large MP3 conversion callback never completes"
    );
    click(instance, &mut store, "quality-mild").await?;
    ensure!(!enabled(store.data().renders.last().unwrap(), "send"), "quality change must invalidate prepared PCM");
    click(instance, &mut store, "prepare").await?;
    click(instance, &mut store, "quality-strong").await?;
    ensure!(!enabled(store.data().renders.last().unwrap(), "send"), "strong quality reused stale PCM");
    click(instance, &mut store, "prepare").await?;
    click(instance, &mut store, "pick-mp3").await?;
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "tone.mp3")
            && enabled(store.data().renders.last().unwrap(), "send"),
        "cancelling replacement picker lost prepared song"
    );
    click(instance, &mut store, "pick-lyrics").await?;
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "tone.lrc"),
        "lyrics result remains pending"
    );
    click(instance, &mut store, "prepare").await?;
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "含歌词"),
        "lyrics were not prepared"
    );
    capture(store.data(), "plugin-prepared")?;
    click(instance, &mut store, "send").await?;
    ensure!(
        !store.data().sent.is_empty(),
        "send callback remains pending"
    );
    ensure!(
        store.data().calls == ["devices", "apps", "register", "launch", "send"],
        "automatic-target callbacks not executed: {:?}",
        store.data().calls
    );
    let packet: serde_json::Value = serde_json::from_str(&store.data().sent[0])?;
    ensure!(packet["op"] == "begin", "not an actual begin packet");
    let ev_idx = instance
        .get_export_index(&mut store, Some(&event), "on-event")
        .unwrap();
    let ev = instance.get_func(&mut store, ev_idx).unwrap();
    let begin = packet;
    ensure!(begin["peak"] == 127, "strong preset did not reach transfer metadata");
    ensure!(
        begin.get("resumeRevision").is_none(),
        "must not request durable resume"
    );
    if recovery == "legacy-blocked" {
        let conflict = serde_json::json!({"tag":"haptic.music.v1","type":"ack","ok":false,
            "id":begin["id"],"seq":0,"op":"begin","error":"存在未完成传输，请在手机继续或中断"});
        let before = store.data().sent.len();
        incoming(instance, &mut store, ev, &conflict, &incoming_format).await?;
        ensure!(
            store.data().sent.len() == before,
            "must not send under or cancel an old id"
        );
        ensure!(
            enabled(store.data().renders.last().unwrap(), "send"),
            "legacy error leaves UI locked"
        );
        ensure!(
            text_contains(store.data().renders.last().unwrap(), "旧续传记录"),
            "legacy error not explained"
        );
        println!("Wasmtime 38.0.4 PASS: legacy checkpoint reported; no old-id reuse or deletion");
        capture(store.data(), "plugin-blocked")?;
        return Ok(());
    }
    let mut current = begin.clone();
    for _ in 0..20 {
        let mut ack = serde_json::json!({"tag":"haptic.music.v1","type":"ack",
            "id":current["id"],"seq":current["seq"],"op":current["op"],"ok":true});
        match current["op"].as_str().unwrap() {
            "begin" => {
                ack["receiptRevision"] = 1.into();
                ack["lyricsRevision"] = 1.into();
                ack["resumeRevision"] = 1.into();
                ack["next"] = 0.into();
                ack["nextSeq"] = 1.into();
                ack["transport"] = serde_json::json!({"revision":1,"chunkBytes":4096,
                    "intervalMs":20,"payloadEncoding":"base64"});
            }
            "chunk" => {
                let data = current["data_b64"].as_str().unwrap();
                let bytes =
                    data.len() / 4 * 3 - data.chars().rev().take_while(|&c| c == '=').count();
                ack["next"] = (current["offset"].as_u64().unwrap() + bytes as u64).into();
            }
            "commit" => {
                ack["saved"] = true.into();
                ack["next"] = begin["bytes"].clone();
                ack["receipt"] = serde_json::json!({"revision":1,"state":"saved","verified":true,
                    "id":begin["id"],"title":begin["title"],"bytes":begin["bytes"],"crc32":begin["crc32"],
                    "peak":begin["peak"],"lyricsRevision":1,"lyricsChars":begin["lyrics"].as_str().unwrap().encode_utf16().count()});
            }
            op => return Err(anyhow!("unexpected packet {op}")),
        }
        let before = store.data().sent.len();
        incoming(instance, &mut store, ev, &ack, &incoming_format).await?;
        if current["op"] == "commit" {
            break;
        }
        if store.data().sent.len() == before {
            let (&id, payload) = store
                .data()
                .timers
                .iter()
                .find(|(_, p)| {
                    serde_json::from_str::<serde_json::Value>(p).unwrap()["kind"] == "next"
                })
                .ok_or_else(|| anyhow!("next-chunk timer not scheduled"))?;
            let payload =
                serde_json::json!({"timerId":id,"kind":"timeout","payload":payload}).to_string();
            store.data_mut().timers.remove(&id);
            store.data_mut().firing = Some(id);
            legacy_call(
                instance,
                &mut store,
                ev,
                &[Val::Enum("timer".into()), Val::String(payload)],
            )
            .await?;
            store.data_mut().firing = None;
        }
        ensure!(
            store.data().sent.len() > before,
            "next chunk was never sent"
        );
        current = serde_json::from_str(store.data().sent.last().unwrap())?;
    }
    ensure!(
        text_contains(store.data().renders.last().unwrap(), "手环已校验保存"),
        "final saved receipt does not reach UI"
    );
    capture(store.data(), "plugin-saved")?;
    println!(
        "Wasmtime 38.0.4 PASS ({incoming_format}, {recovery}): legacy V3 dispatch → cancel / invalid / 8 MiB MP3 / LRC → conversion → automatic target → timed chunks → verified receipt"
    );
    Ok(())
}
