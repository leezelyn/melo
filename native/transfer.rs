use base64::Engine;
use serde_json::{Value, json};

#[derive(Default)]
pub struct Sender {
    pub phase: String,
    pub status: String,
    pub id: String,
    pub addr: String,
    pub title: String,
    pub bytes: Vec<u8>,
    pub lyrics: String,
    pub crc: u32,
    pub peak: u8,
    pub offset: usize,
    pub seq: u64,
    pub serial: u64,
    pub pending: Option<Value>,
    pub attempts: u8,
    pub due: Option<(u64, String)>,
    pub chunk: usize,
    pub interval: u64,
    pub encoding: String,
    pub receipt: Option<Value>,
    offer_chunk: usize,
    offer_interval: u64,
}
impl Sender {
    pub fn active(&self) -> bool {
        matches!(
            self.phase.as_str(),
            "starting" | "sending" | "saving" | "cancelling"
        )
    }
    pub fn unfinished(&self) -> bool {
        !self.phase.is_empty()
            && !matches!(
                self.phase.as_str(),
                "idle" | "saved" | "cancelled" | "blocked"
            )
    }
    pub fn progress(&self) -> usize {
        if self.phase == "saved" {
            100
        } else if self.bytes.is_empty() {
            0
        } else {
            (self.offset * 100 / self.bytes.len()).min(99)
        }
    }
    pub fn snapshot(&self) -> Value {
        json!({"phase":self.phase,"status":self.status,"offset":self.offset,"total":self.bytes.len(),"progress":self.progress(),"serial":self.serial,"pending":self.pending,"due":self.due,"receipt":self.receipt})
    }
    pub fn start(
        &mut self,
        id: String,
        addr: String,
        title: String,
        bytes: Vec<u8>,
        lyrics: String,
        profile: &str,
        peak: u8,
    ) -> Result<(), String> {
        if self.unfinished() {
            return Err("已有未结束任务，请继续或取消".into());
        }
        if id.len() != 32
            || !id
                .bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
            || addr.is_empty()
            || title.trim().is_empty()
            || title.encode_utf16().count() > 64
            || title.chars().any(|c| c < ' ')
            || !(2..=3600000).contains(&bytes.len())
            || !matches!(peak, 64 | 127)
            || bytes.iter().any(|&v| (v as i8 as i16).abs() > peak as i16)
        {
            return Err("歌曲信息无效".into());
        }
        let (chunk, interval) = match profile {
            "stable" => (1024, 50),
            "balanced" => (4096, 20),
            "fast" => (16384, 0),
            _ => return Err("传输速度无效".into()),
        };
        let serial = self.serial + 1;
        *self = Self {
            phase: "starting".into(),
            status: "等待播放器自动回应，无需在手环确认".into(),
            id,
            addr,
            title,
            crc: crc32fast::hash(&bytes),
            peak,
            bytes,
            lyrics,
            serial,
            offer_chunk: chunk,
            offer_interval: interval,
            ..Default::default()
        };
        self.request(self.begin());
        Ok(())
    }
    fn message(&self, op: &str) -> Value {
        json!({"tag":"haptic.music.v1","id":self.id,"seq":self.seq,"op":op})
    }
    fn begin(&self) -> Value {
        let mut m = self.message("begin");
        m["seq"] = json!(0);
        m["title"] = json!(self.title);
        m["bytes"] = json!(self.bytes.len());
        m["crc32"] = json!(self.crc);
        m["rate"] = json!(12000);
        m["encoding"] = json!("s8");
        m["peak"] = json!(self.peak);
        m["transport"] = json!({"chunkBytes":self.offer_chunk,"intervalMs":self.offer_interval,"payloadEncoding":"base64"});
        if !self.lyrics.is_empty() {
            m["lyrics"] = json!(self.lyrics)
        }
        m
    }
    fn request(&mut self, m: Value) {
        self.serial += 1;
        self.attempts = 1;
        let ms = if matches!(m["op"].as_str(), Some("commit" | "receipt")) {
            60000
        } else {
            20000
        };
        self.pending = Some(m);
        self.due = Some((ms, "timeout".into()))
    }
    fn fail(&mut self, reason: impl Into<String>, uncertain: bool) {
        self.serial += 1;
        self.due = None;
        self.phase = if uncertain { "unconfirmed" } else { "paused" }.into();
        self.status = reason.into()
    }
    pub fn timeout(&mut self, serial: u64, kind: &str) {
        if serial != self.serial || !self.active() {
            return;
        }
        if kind == "next" {
            self.next();
            return;
        }
        if self.pending.is_none() {
            return;
        }
        if self.attempts < 3 {
            self.attempts += 1;
            self.serial += 1;
            return;
        }
        let op = self.pending.as_ref().unwrap()["op"].as_str().unwrap_or("");
        if op == "commit" {
            self.request(self.message("receipt"));
            return;
        }
        let uncertain = matches!(op, "receipt" | "cancel");
        self.fail(
            if uncertain {
                "结果未确认，请继续查询或重试取消"
            } else {
                "手环回执超时，请保持播放器打开后继续"
            },
            uncertain,
        )
    }
    pub fn receive(&mut self, a: Value) {
        if !self.active() || a["tag"] != "haptic.music.v1" {
            return;
        }
        let Some(p) = self.pending.clone() else {
            return;
        };
        if a["id"] != p["id"] {
            return;
        }
        if a["type"] == "cancelled" {
            self.finish_cancel();
            return;
        }
        if a["type"] != "ack" || a["seq"] != p["seq"] || a["op"] != p["op"] {
            return;
        }
        self.serial += 1;
        self.due = None;
        let op = p["op"].as_str().unwrap_or("");
        if a["cancelled"] == true && op != "cancel" {
            self.finish_cancel();
            return;
        }
        if a["ok"] != true {
            if op == "begin"
                && (a.get("pendingTransfer").is_some()
                    || a["error"]
                        .as_str()
                        .is_some_and(|e| e.contains("未完成传输")))
            {
                self.phase = "blocked".into();
                self.status =
                    "手环仍有旧续传记录，请在手环的‘传歌’页点击‘取消旧任务’，完成后重新传歌".into();
                self.pending = None;
                self.id.clear();
            } else if op == "commit" {
                self.request(self.message("receipt"))
            } else {
                self.fail(
                    a["error"].as_str().unwrap_or("手环拒绝操作"),
                    a["unconfirmed"] == true,
                )
            }
            return;
        }
        if a["saved"] == true {
            if let Err(e) = self.accept_receipt(&a) {
                self.fail(e, true)
            }
            return;
        }
        if op == "begin" {
            let result = (|| -> Result<(), String> {
                if a["receiptRevision"] != 1 {
                    return Err("请更新支持保存校验的环间音乐 RPK".into());
                }
                if !self.lyrics.is_empty() && a["lyricsRevision"] != 1 {
                    return Err("手环不支持歌词".into());
                }
                let t=a.get("transport").cloned().unwrap_or(json!({"revision":1,"chunkBytes":1024,"intervalMs":self.offer_interval,"payloadEncoding":"hex"}));
                let chunk = t["chunkBytes"].as_u64().ok_or("分块大小无效")? as usize;
                let interval = t["intervalMs"].as_u64().ok_or("分块间隔无效")?;
                let encoding = t["payloadEncoding"].as_str().ok_or("分块编码无效")?;
                if t["revision"] != 1
                    || !(256..=self.offer_chunk).contains(&chunk)
                    || !matches!(encoding, "hex" | "base64")
                    || encoding == "hex" && chunk > 1024
                    || !(self.offer_interval..=1000).contains(&interval)
                {
                    return Err("手环协商参数无效".into());
                }
                let next = a["next"].as_u64().ok_or("接收位置无效")? as usize;
                let seq = match a.get("nextSeq") {
                    Some(value) => value.as_u64().ok_or("接收序号无效")?,
                    None => 1,
                };
                if next != 0 || seq != 1 {
                    return Err("本次从头传输，手环返回了旧位置，已停止发送".into());
                }
                self.chunk = chunk;
                self.interval = interval;
                self.encoding = encoding.into();
                self.offset = next;
                self.seq = seq;
                Ok(())
            })();
            if let Err(e) = result {
                self.fail(e, false)
            } else {
                self.next()
            }
        } else if op == "chunk" {
            let expected = self.offset + (self.bytes.len() - self.offset).min(self.chunk);
            if a["next"].as_u64() != Some(expected as u64) {
                self.fail("手环确认的分块长度不符", false);
                return;
            }
            self.offset = expected;
            self.seq += 1;
            if self.offset < self.bytes.len() && self.interval > 0 {
                self.pending = None;
                self.phase = "sending".into();
                self.due = Some((self.interval, "next".into()))
            } else {
                self.next()
            }
        } else if op == "cancel" {
            if a["cancelled"] == true && a["unconfirmed"] != true {
                self.finish_cancel()
            } else {
                self.fail("手环中断结果未确认", true)
            }
        } else {
            self.fail("保存结果未确认，请继续查询回执", true)
        }
    }
    fn accept_receipt(&mut self, a: &Value) -> Result<(), String> {
        let r = &a["receipt"];
        if r["revision"] != 1
            || r["state"] != "saved"
            || r["verified"] != true
            || r["id"] != self.id
            || r["title"] != self.title
            || r["bytes"].as_u64() != Some(self.bytes.len() as u64)
            || r["crc32"].as_u64() != Some(self.crc as u64)
            || a["next"].as_u64() != Some(self.bytes.len() as u64)
            || r.get("peak").is_some_and(|x| *x != self.peak)
            || self.peak != 64 && r.get("peak").is_none()
            || !self.lyrics.is_empty()
                && (r["lyricsRevision"] != 1
                    || r["lyricsChars"].as_u64() != Some(self.lyrics.encode_utf16().count() as u64))
        {
            return Err("手环保存回执与歌曲不匹配".into());
        }
        self.serial += 1;
        self.due = None;
        self.pending = None;
        self.offset = self.bytes.len();
        self.phase = "saved".into();
        self.status = "传输完成 · 手环已校验保存".into();
        let mut receipt = r.clone();
        if let Some(fields) = receipt.as_object_mut() {
            fields.remove("id");
        }
        self.receipt = Some(receipt);
        self.id.clear();
        Ok(())
    }
    fn next(&mut self) {
        if self.offset == self.bytes.len() {
            self.phase = "saving".into();
            self.status = "音频已接收，等待手环校验保存".into();
            self.request(self.message("commit"));
            return;
        }
        self.phase = "sending".into();
        self.status = format!("正在传歌 · {}%", self.progress());
        let b = &self.bytes[self.offset..(self.offset + self.chunk).min(self.bytes.len())];
        let mut m = self.message("chunk");
        m["offset"] = json!(self.offset);
        if self.encoding == "base64" {
            m["data_b64"] = json!(base64::engine::general_purpose::STANDARD.encode(b));
            m["crc32"] = json!(crc32fast::hash(b))
        } else {
            m["hex"] = json!(b.iter().map(|v| format!("{v:02x}")).collect::<String>())
        }
        self.request(m)
    }
    pub fn resume(&mut self) -> Result<(), String> {
        if !matches!(self.phase.as_str(), "paused" | "unconfirmed") {
            return Err("没有可继续的任务".into());
        }
        let previous = self.pending.clone();
        self.phase = "starting".into();
        self.status = "正在重试当前传输".into();
        match previous.as_ref().and_then(|m| m["op"].as_str()) {
            Some("commit" | "receipt") => {
                self.phase = "saving".into();
                self.request(self.message("receipt"))
            }
            Some("cancel") => {
                self.phase = "cancelling".into();
                self.request(previous.unwrap())
            }
            _ => {
                if let Some(p) = previous {
                    self.request(p)
                } else {
                    self.next()
                }
            }
        }
        Ok(())
    }
    fn finish_cancel(&mut self) {
        self.serial += 1;
        self.pending = None;
        self.due = None;
        self.phase = "cancelled".into();
        self.id.clear();
        self.status = "手环已确认中断，已有歌曲保留".into()
    }
    pub fn cancel(&mut self) {
        if !self.unfinished() {
            return;
        }
        self.phase = "cancelling".into();
        self.status = "等待手环确认中断".into();
        self.request(self.message("cancel"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enhanced_audio_declares_and_verifies_its_actual_peak() {
        let mut s = Sender::default();
        s.start(
            "1".repeat(32),
            "device".into(),
            "增强".into(),
            vec![127, 129],
            String::new(),
            "balanced",
            127,
        )
        .unwrap();
        assert_eq!(s.pending.as_ref().unwrap()["peak"], 127);
        let mut receipt = json!({"next":2,"receipt":{"revision":1,"state":"saved","verified":true,"id":s.id,"title":s.title,"bytes":2,"crc32":s.crc,"peak":64}});
        assert!(s.accept_receipt(&receipt).is_err());
        receipt["receipt"]["peak"] = json!(127);
        assert!(s.accept_receipt(&receipt).is_ok());
    }
    fn sender() -> Sender {
        let mut s = Sender::default();
        s.start(
            "1".repeat(32),
            "device".into(),
            "测试".into(),
            vec![1; 9017],
            String::new(),
            "balanced",
            64,
        )
        .unwrap();
        s
    }
    fn ack(s: &Sender, more: Value) -> Value {
        let p = s.pending.as_ref().unwrap();
        let mut a = json!({"tag":"haptic.music.v1","type":"ack","id":s.id,"seq":p["seq"],"op":p["op"],"ok":true});
        a.as_object_mut()
            .unwrap()
            .extend(more.as_object().unwrap().clone());
        a
    }
    fn ready(s: &Sender) -> Value {
        ack(
            s,
            json!({"next":0,"nextSeq":1,"resumeRevision":1,"receiptRevision":1,"transport":{"revision":1,"chunkBytes":4096,"intervalMs":20,"payloadEncoding":"base64"}}),
        )
    }
    #[test]
    fn only_ack_advances() {
        let mut s = sender();
        assert_eq!(s.offset, 0);
        let a = ready(&s);
        s.receive(a);
        assert_eq!(s.pending.as_ref().unwrap()["op"], "chunk");
        assert_eq!(s.offset, 0);
        let a = ack(&s, json!({"next":4095}));
        s.receive(a);
        assert_eq!(s.phase, "paused");
        assert_eq!(s.offset, 0)
    }
    #[test]
    fn retries_are_identical_and_bounded() {
        let mut s = sender();
        let first = s.pending.clone();
        for _ in 0..3 {
            s.timeout(s.serial, "timeout")
        }
        assert_eq!(s.phase, "paused");
        assert_eq!(s.pending, first);
        assert_eq!(s.attempts, 3)
    }
    #[test]
    fn wrong_session_is_ignored() {
        let mut s = sender();
        let mut a = ready(&s);
        a["id"] = json!("2".repeat(32));
        s.receive(a);
        assert_eq!(s.phase, "starting")
    }
    #[test]
    fn cancel_ignores_late_chunk_ack() {
        let mut s = sender();
        let a = ready(&s);
        s.receive(a);
        let late = ack(&s, json!({"next":4096}));
        s.cancel();
        s.receive(late);
        assert_eq!(s.pending.as_ref().unwrap()["op"], "cancel");
        let a = ack(&s, json!({"cancelled":true}));
        s.receive(a);
        assert_eq!(s.phase, "cancelled")
    }
    #[test]
    fn forged_receipt_cannot_succeed() {
        let mut s = sender();
        let a = ack(
            &s,
            json!({"saved":true,"next":9017,"receipt":{"revision":1,"state":"saved","verified":true,"id":"1".repeat(32),"title":"测试","bytes":9017,"crc32":0}}),
        );
        s.receive(a);
        assert_eq!(s.phase, "unconfirmed")
    }
    #[test]
    fn retry_only_replays_the_current_packet() {
        let mut s = sender();
        let a = ready(&s);
        s.receive(a);
        let packet = s.pending.clone();
        for _ in 0..3 {
            s.timeout(s.serial, "timeout");
        }
        s.resume().unwrap();
        assert_eq!(s.pending, packet);
        assert!(s.pending.as_ref().unwrap().get("resumeRevision").is_none());
    }
    #[test]
    fn new_tasks_do_not_request_durable_resume() {
        let s = sender();
        assert!(s.pending.unwrap().get("resumeRevision").is_none());
    }
    #[test]
    fn a_legacy_checkpoint_is_not_adopted_or_cancelled() {
        let mut s = sender();
        let a = ack(
            &s,
            json!({"ok":false,"error":"存在另一首未完成传输", "pendingTransfer":{"id":"2".repeat(32)}}),
        );
        s.receive(a);
        assert_eq!(s.phase, "blocked");
        assert!(s.id.is_empty());
        assert!(s.pending.is_none());
        assert!(!s.unfinished());
        s.cancel();
        assert!(s.pending.is_none());
    }
    #[test]
    fn cancelled_tasks_discard_their_temporary_id() {
        let mut s = sender();
        s.cancel();
        let a = ack(&s, json!({"cancelled":true}));
        s.receive(a);
        assert_eq!(s.phase, "cancelled");
        assert!(s.id.is_empty());
        assert!(s.pending.is_none());
    }
}
