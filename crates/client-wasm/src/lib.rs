//! `client-core`를 브라우저에 내보내는 얇은 래퍼. 로직은 없습니다.
//! 브라우저(web/)는 프레임마다 set_* → update → render_json 순서로 부르고, 받은 메시지는 해석하지 않고 on_message에 넘깁니다.

use client_core::{Client, JoinError};
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct WasmClient {
    inner: Client,
}

#[wasm_bindgen]
impl WasmClient {
    #[wasm_bindgen(constructor)]
    #[allow(clippy::new_without_default)] // wasm-bindgen 생성자는 Default로 내보낼 수 없습니다.
    pub fn new() -> WasmClient {
        WasmClient {
            inner: Client::new(),
        }
    }

    /// 입장 메시지. 방 코드나 이름이 틀리면 오류 코드("bad_room_code", "bad_name")를 메시지로 하는 예외를 던집니다.
    pub fn join_message(&mut self, room: &str, name: &str) -> Result<String, JsError> {
        self.inner.join_message(room, name).map_err(|e| {
            JsError::new(match e {
                JoinError::BadRoomCode => "bad_room_code",
                JoinError::BadName => "bad_name",
            })
        })
    }

    /// 서버가 보낸 글자 메시지 하나. 해석하지 못하면 오류 설명을 돌려줍니다.
    pub fn on_message(&mut self, text: &str, now_ms: f64) -> Option<String> {
        self.inner.on_message(text, now_ms).err()
    }

    pub fn set_viewport(&mut self, width_px: f64, height_px: f64) {
        self.inner.set_viewport(width_px, height_px);
    }

    pub fn set_keys(&mut self, up: bool, down: bool, left: bool, right: bool) {
        self.inner.set_keys(up, down, left, right);
    }

    pub fn set_pointer(&mut self, x_px: f64, y_px: f64) {
        self.inner.set_pointer(x_px, y_px);
    }

    /// 시간을 진행하고 서버로 보낼 메시지들을 돌려줍니다.
    pub fn update(&mut self, now_ms: f64) -> Vec<String> {
        self.inner.update(now_ms)
    }

    /// 화면에 그릴 상태 (JSON). 형식은 client-core의 `RenderState`입니다.
    pub fn render_json(&mut self) -> String {
        self.inner.render_json()
    }

    /// 맵 타일 (JSON). render_json의 map_rev가 바뀔 때만 부르면 됩니다.
    pub fn map_json(&self) -> String {
        self.inner.map_json()
    }
}

/// 정해진 입력열을 돌린 결과 한 줄. 네이티브 `tools determinism`의 출력과 같아야 합니다.
#[wasm_bindgen]
pub fn determinism_line(
    balance_toml: &str,
    map_text: &str,
    seed: u32,
    ticks: u32,
) -> Result<String, JsError> {
    shared::scenario::run(balance_toml, map_text, u64::from(seed), ticks)
        .map(|r| r.to_line())
        .map_err(|e| JsError::new(&e))
}
