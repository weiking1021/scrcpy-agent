# scrcpy-agent

[English](README.md) | **繁體中文**

以 [scrcpy](https://github.com/Genymobile/scrcpy) 為基礎的桌面服務。常駐於系統匣，在區域網路上找到 Android 裝置，
重新連線已配對的裝置，並開啟它們的畫面。以 JavaScript 撰寫的 Addon 可以擴充功能。

![裝置清單：已配對的裝置與在網路上發現的裝置](docs/images/devices.png)

## 功能

- **裝置探索與連線。** 透過 mDNS 與 adb 追蹤無線偵錯。只連線已配對的裝置，不連同一網路上的其他裝置。已配對的裝置停止
  廣播時（例如螢幕鎖定），會改試它最後一次的已知位址。也支援 USB 裝置。
- **畫面。** 整個裝置的畫面，或單一 App 的視窗（在獨立的顯示器上），附浮動工具列（返回、主畫面、旋轉……以及 Addon 加入的
  按鈕）。顯示哪些按鈕與其順序可在設定中調整。
- **檔案。** 傳送檔案到裝置、在裝置上開啟檔案；可從程式內操作，也可從檔案管理員的右鍵選單操作
  （Windows 檔案總管；Linux 的 Dolphin、Nemo、Nautilus）。
- **系統匣。** 關閉主視窗時會隱藏到系統匣，程式繼續執行（設定 `close_to_tray` 可關閉此行為）。程式也可以隨登入啟動。
- **外觀。** 主題（CSS）與語言包，內建英文與繁體中文。
- **Addon。** 含 `addon.json` 與 `main.js` 的資料夾，在沙盒化的 JavaScript 引擎（QuickJS）中執行，只擁有安裝時授予的
  權限。Addon 可以加入選單項目與工具列按鈕、回應事件、宣告設定（由程式畫出表單）、在裝置上執行 shell 指令、在電腦上
  啟動程式，以及接收畫面的即時影像（例如用於影像辨識）。

scrcpy、adb 與 scrcpy server 不包含在內。第一次執行時，程式會先請使用者確認，再從官方 GitHub release 下載 scrcpy
（壓縮包同時含這三者，因此版本一定相符），並先顯示檔名、大小與 SHA-256。預設版本是 v5.0.1，即程式測試所用的版本；
版本可在設定中更換。

## 截圖

![裝置上的 App，各自開在獨立的視窗](docs/images/app-windows.png)

![裝置的詳細頁：App、詳細資料與裝置設定](docs/images/device-details.png)

![從檔案管理員的右鍵選單傳送檔案到裝置](docs/images/context-menu.png)

## 安裝

安裝包放在 [Releases](https://github.com/weiking1021/scrcpy-agent/releases) 頁面：

| 系統 | 安裝包 |
|---|---|
| Windows | 安裝程式（`…_x64-setup.exe`；僅限目前使用者，不需系統管理員）或免安裝 zip（程式旁放 `portable.txt` 時，資料存在程式旁） |
| Linux（x86_64） | `.deb` 套件（需要 WebKitGTK 4.1、GTK 3 與 Ayatana app indicator；`apt install ./…deb` 會一併安裝）或單一執行檔 |

裝置需求：Android 11 以上，開啟**開發人員選項 → 無線偵錯**，並在程式中配對一次（「配對裝置」）。以 USB 連接並開啟
USB 偵錯的裝置不需要配對。

## Addon

- 撰寫方式：[docs/ADDONS.zh-TW.md](docs/ADDONS.zh-TW.md)（[English](docs/ADDONS.md)）；可執行的範例在
  [docs/example-addon/](docs/example-addon/)。
- TypeScript 型別：套件 `scrcpy-agent-addon-api`，由 `npm run addon-types` 在 `out/` 產生 `.tgz`（不發佈到 npm）。
- 主題：[docs/theming/THEMING.md](docs/theming/THEMING.md)。

Addon 只能使用安裝時勾選的權限。`run_process`（執行電腦上的程式）與 `screen_capture`（取得裝置畫面）會附上警告。

## 從原始碼建置

需求：[Rust](https://rustup.rs/)（stable）、Node.js，以及
[Tauri 2 需要的系統函式庫](https://tauri.app/start/prerequisites/)（Windows 為 MSVC 建置工具與 WebView2；
Linux 為 `libwebkit2gtk-4.1-dev`、`libayatana-appindicator3-dev` 等）。

```
npm install
npm run tauri dev        # 執行，檔案變動時自動重編
npm run tauri build      # 產生此系統的程式與安裝程式
npm run package          # Docker：Linux 與 Windows 的安裝包及 Addon 型別，輸出到 out/
npm run package -- --sequential  # 本機記憶體較少時，依序建置 Linux 與 Windows
```

測試使用假的 adb 與假的 scrcpy（`src-tauri/examples/`），不需要實體裝置：

```
cd src-tauri && cargo clippy --all-targets && cargo test
npx tsc --noEmit && npm run check:locales
```

## 回報問題

問題請回報到 [Issues](https://github.com/weiking1021/scrcpy-agent/issues) 頁面。附上程式版本、作業系統、重現步驟，以及除錯控制台
中相關的幾行，回報會最有用。貼出的內容中，裝置序號、位址與名稱需先移除。

## 授權

[MIT](LICENSE)。scrcpy 是 Genymobile 的專案，採 Apache License 2.0；本程式與其無隸屬關係。
