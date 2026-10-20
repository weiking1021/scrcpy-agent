# 撰寫 Addon

[English](ADDONS.md) | **繁體中文**

本文件供 Addon 作者參考。

## 資料夾

一個 Addon 是一個資料夾，至少包含兩個檔案：

```
my-add-on/
  addon.json   Addon 的識別資訊與要求的權限
  main.js      Addon 的程式碼
```

其餘檔案不限。入口固定是 `main.js`，不能改名。`main.js` 是單一檔案，大小上限 4 MiB（超過時不會被讀取，Addon 不會啟動）。
使用 TypeScript 或套件時，需事先以工具鏈（`tsc`、`esbuild` 等）打包進這個檔案，目標語法為 ES2022。程式不編譯
TypeScript，也沒有 `require`。

最小的 Addon：

```json
{ "id": "hello", "name": "Hello", "version": "1.0.0", "api": 1 }
```

```js
host.deviceMenu.register({
  id: "say-hello",
  label: "Say hello",
  onClick: (device) => host.log("hello from " + device),
});
```

## addon.json

```json
{
  "id": "my-addon",
  "name": "My add-on",
  "version": "1.0.0",
  "api": 1,
  "permissions": ["device_shell"],
  "platforms": ["windows", "linux"]
}
```

| 欄位 | 必填 | 規則 |
|---|---|---|
| `id` | 是 | 1 到 64 個英文字母、數字、`_`、`-`。安裝後不能改。 |
| `name` | 是 | 1 到 60 個字元，不能有控制字元；頭尾空白會被去掉。 |
| `version` | 是 | 字串，1 到 40 個字元。只用來顯示，程式不比較版本。 |
| `api` | 是 | 1 以上的整數：Addon 所針對的 Addon 介面版本。程式支援 `1`；更新的版本不會執行。 |
| `permissions` | 否 | 要求的權限清單，不能重複；沒寫表示不要求任何權限。 |
| `platforms` | 否 | 支援的系統（`windows`、`linux`、`android`），至少一個，不能重複；沒寫表示全部。 |
| `settings` | 否 | 使用者可為這個 Addon 調整的設定（見下一節）；沒寫表示沒有。宣告不合格時，整個 `addon.json` 視為不合格。 |

其他欄位會被忽略。超過 64 KiB 的 `addon.json` 不會被讀取。任一欄位不合格時，安裝會被拒絕，原因會指出是哪個欄位。

### 設定（`settings`）

在 `addon.json` 宣告 `settings` 後，程式會在設定頁該 Addon 那一列加上「設定」按鈕，畫出表單、檢查使用者填的值並保存。
`main.js` 以 `host.settings.get("id")` 讀取，值改變時收到事件 `settings.changed`。一個設定是一個物件：`id` 與
`label` 必填，`hint`（表單中的一行說明）選填，`type` 為下列四種之一：

| `type` | 必填欄位 | 值 |
|---|---|---|
| `boolean` | `default`（true 或 false） | 開關 |
| `number` | `default`、`min`、`max`（皆為整數，`min` ≤ `default` ≤ `max`；帶小數的數字如 `5.0` 會被拒絕） | 整數 |
| `text` | `default`（文字）、`max_length`（1 到 1000） | 單行文字（不能有換行等控制字元）；`default` 同樣須符合這些規則與 `max_length` |
| `choice` | `options`（`{ "value", "label" }` 的清單，1 到 50 個）、`default`（其中一個 `value`） | 其中一個選項的 `value` |

```json
"settings": [
  { "id": "threshold", "type": "number", "label": "Threshold", "default": 5, "min": 0, "max": 100 },
  { "id": "verbose", "type": "boolean", "label": "Write more to the console", "default": false },
  { "id": "mode", "type": "choice", "label": "Mode", "default": "fast",
    "options": [{ "value": "fast", "label": "Fast" }, { "value": "slow", "label": "Slow" }] }
]
```

規則：

- 設定的 `id` 與選項的 `value`，規則同 Addon 的 `id`（1 到 64 個英文字母、數字、`_`、`-`）。設定的 `id` 不能重複，同一個 `choice`
  內的 `value` 也不能重複。
- 最多 50 個設定。`label`（包含選項的 `label`）為單行的 1 到 100 個字元。`hint` 若有，為單行的 1 到 300 個字元。
- 宣告中的 `default` 必須是該設定允許的值。
- 保存的值每次讀取時都對照現在的宣告檢查。宣告的範圍改變而保存的值不再符合時，該設定退回預設值，其他設定不受影響。
- 使用者填入不允許的值時，表單會說明原因並放回原本的值。`main.js` 讀到的永遠是合法的值。

## main.js 能使用的功能

`main.js` 在程式內嵌的 JavaScript 引擎（QuickJS）中執行。每個 Addon 有自己的執行環境和執行緒，彼此看不到對方。執行環境
最多使用 32 MiB 記憶體，超過時腳本會被中止。語言本身的功能都可用（`Promise`、`async`/`await`、`JSON`、`Map` 等）。
沒有檔案、網路，也沒有 `require`、`process`、`fetch`、`Buffer`。凡是會接觸引擎之外的功能，都列在下面。

### 執行時機

- `main.js` 在啟動時執行一次：程式啟動時、Addon 安裝時、使用者重新決定它的權限時（這時會先停止再重新啟動）。
  `main.js` 回傳之後，以 Promise 或計時器啟動的工作會繼續。`main.js` 執行超過 5 秒會被中斷，Addon 不會啟動，設定頁會說明原因。
- 註冊的 callback（選單被點擊、計時器）一次一個、依到達順序執行。每個 callback 最多 5 秒（含等待），超過會被中斷並記錄，
  之後的 callback 照常執行。
- Addon 被移除或重新啟動時，它註冊的項目與計時器都會被清除。

### `host`

| | 需要權限 | 說明 |
|---|---|---|
| `host.log(...值)` | 否 | 寫一行到除錯控制台，標明是這個 Addon。值會轉成文字；一行最多 2000 字元，超過會截斷並以 `…` 結尾。 |
| `host.deviceMenu.register({ id, label, onClick })` | 否 | 在裝置清單中該裝置的卡片上加一個按鈕。`id` 與 `label` 是文字（`id` 規則同 Addon 的 `id`，`label` 為 1 到 64 個字元），`onClick(device)` 是函式，`device` 是裝置的 id（程式給的不透明文字；與 `deviceShell` 和事件使用的相同）。 |
| `host.appMenu.register({ id, label, onClick })` | 否 | 在 App 的選單加一個項目。規則同上，`onClick(device, app)`，`app` 是套件名稱。 |
| `host.toolbar.register({ id, label, onClick })` | 否 | 在畫面的工具列加一個按鈕（通用圖示，`label` 是提示文字）。`onClick(device, app)`：App 視窗的 `app` 是套件名稱，裝置本身的畫面為 `null`。 |
| `await host.deviceShell(device, command)` | `device_shell` | 在裝置的 shell 中原樣執行指令，回傳標準輸出（最多 1 MiB）。結束碼不是 0 時丟出 `DeviceError`（`code` 為 `command_failed`）。 |
| `host.process.spawn({ command, args, onLine, onExit })` | `run_process` | 在這台電腦上啟動常駐程式，回傳控制代碼 `{ kill(), closeInput() }`。`command` 與每個 `args` 各自是文字，原樣交給作業系統（不經過 shell）。`onLine(行)` 收到它標準輸出的每一行（單行最多 64 KiB，超過的部分丟棄），`onExit(結束碼)` 在它結束時呼叫；被程式結束或被訊號中止時，結束碼為 `null`。每個 Addon 最多同時 4 個；Addon 停止、停用或移除時一併結束。啟動失敗時丟出 `ProcessError`，`code` 為 `invalid_command`、`not_found`、`not_allowed`、`too_many` 或 `failed`。 |
| `host.screen.pipeTo(控制代碼, device, app?)` | `screen_capture` | 把裝置畫面的即時影片（H.264、Annex B，維持裝置編碼的原樣，不經過 Addon 的 JS）持續寫入該程式的標準輸入。程式讀取速度慢於畫面時，會丟棄畫面直到下一個關鍵幀。裝置沒有可跟隨的開啟畫面時丟出 `ScreenError`（`code` 為 `no_screen`）；控制代碼不屬於這個 Addon 的程式時為 `no_process`。 |
| `host.settings.get(id)` | 否 | 使用者為這個 Addon 設定的值（`addon.json` 中宣告的設定）：布林、整數或文字。未宣告的 `id` 回傳 `undefined`；`id` 不是文字時為 `TypeError`。從未設定過的設定為宣告的預設值。 |
| `host.addon.folder()` | 否 | 這個 Addon 安裝來源資料夾的路徑（`addon.json`、`main.js` 所在），依系統的寫法（Windows 為 `C:\...`）。隨 Addon 附的程式位於該處，可交給 `host.process.spawn` 啟動。路徑不是合法文字，或 Addon 已不在已安裝清單時，丟出 `AddonFolderError`。 |

三個註冊位置各有自己的名字空間（同一個 `id` 可在各處各註冊一次）。重複註冊，或 `id`、`label` 不合規則時，丟出名為
`MenuRegisterError` 的錯誤，`code` 為 `invalid_id`、`invalid_label` 或 `already_registered`。Addon 被停用或移除時，
程式會一併收回它註冊的所有項目與啟動的所有程式。

### 事件：`host.on(名稱, 函式)`

程式中發生的事會通知有訂閱的 Addon。事件不需要權限，內容中沒有裝置的序號與網路位址。`host.on` 回傳取消訂閱的函式
（呼叫兩次也沒問題）。同一個事件有多個訂閱時，依訂閱順序逐一執行；其中一個出錯不影響其他的，錯誤會連同事件名稱寫到
除錯控制台。名稱不認得時得到名為 `UnknownEvent` 的錯誤（有 `event` 欄位）；名稱或函式的型別不對時為 `TypeError`。

| 事件 | 內容 | 時機 |
|---|---|---|
| `device.online` | `{ device, name }` | 裝置變成可使用 |
| `device.offline` | `{ device, name }` | 可使用的裝置離線、消失，或不再可使用（例如未授權） |
| `device.added` | `{ device, name }` | 這台電腦第一次認得這台裝置（不是 adb 的配對，那發生在手機端；以 USB 連接的裝置也算） |
| `screen.started` | `{ device, app }` | 畫面開啟；`app` 是 App 視窗的套件名，整台裝置的畫面為 `null` |
| `screen.ended` | `{ device, app }` | 畫面關閉 |
| `files.sent` | `{ device, name, ok, error }` | 檔案傳送結束（傳送中的進度不是事件）；失敗時 `error` 是錯誤碼（例如 `not_online`） |
| `files.opened` | `{ device, file, ok, error }` | 檔案在裝置上開啟（或失敗） |
| `settings.changed` | `{ key, value }` | 使用者改了這個 Addon 的一個設定（保存成功後才通知；只通知這個 Addon，不是廣播） |

- `device` 是裝置的 id（與 `deviceShell` 使用的相同），`name` 是程式顯示的名稱（使用者自訂的名稱優先）。
- 裝置事件來自每次輪詢前後的裝置清單比對，因此比實際發生晚一到兩秒。
- Addon 啟動時會補送現況：每台目前可使用的裝置一個 `device.online`，每個開啟中的畫面一個 `screen.started`（不補
  `device.added`）。重新載入時也會補送。
- Addon 忙碌時最多保留 100 個等待處理的事件，超過時丟棄最舊的，並在控制台記一行寫明丟棄的數量。

### `console`

`console.log`、`info`、`debug`（資訊）、`warn`（警告）、`error`（錯誤）與 `host.log` 一樣寫到除錯控制台，等級對應各自的名稱。

### 計時器

`setTimeout`、`clearTimeout`、`setInterval`、`clearInterval`，用法同瀏覽器。同時最多 100 個計時器；`setInterval` 最短
10 毫秒（更短的視為 10）；延遲最長一天。

### 錯誤

呼叫失敗時丟出一般的 `Error`，其 `name` 為下列之一，可用 `try/catch` 依 `name` 與其他欄位判斷：

| `name` | 其他欄位 | 時機 |
|---|---|---|
| `PermissionDenied` | `permission`：權限名 | 呼叫需要權限的 API，但使用者沒有授予。 |
| `DeviceError` | `code`：核心的錯誤碼（例如 `not_online`、`unknown_device`、`command_failed`） | 裝置上的操作失敗。 |
| `MenuRegisterError` | `code`：`invalid_id`、`invalid_label`、`already_registered` | 註冊的選單項目不被接受。 |
| `ProcessError` | `code`：`invalid_command`、`not_found`、`not_allowed`、`too_many`、`failed` | 程式沒有啟動。 |
| `ScreenError` | `code`：`no_screen`、`no_process` | 畫面沒有傳給程式。 |
| `UnknownEvent` | `event`：給定的名稱 | `host.on` 的名稱不是事件。 |
| `TimerLimit` | | 計時器太多。 |
| `AddonFolderError` | | `host.addon.folder()` 不知道這個 Addon 的資料夾。 |
| `TypeError` | | 參數型別不對（例如 `onClick` 不是函式）。 |

沒有被接住的錯誤依發生位置處理：在 callback 裡，會記為錯誤等級的一行（含錯誤與堆疊），只中止那一個 callback；在
`main.js` 最上層，Addon 不會啟動；沒人處理的被拒絕的 Promise，會記為錯誤。一個 Addon 的錯誤不會影響程式，也不會影響其他
Addon。

## 權限

| 名稱 | 意義 |
|---|---|
| `device_shell` | 在裝置上執行 shell 指令（`host.deviceShell`）。 |
| `run_process` | 在這台電腦上啟動程式（`host.process.spawn`）。程式能做到使用者在這台電腦上能做的任何事，因此安裝時會顯示明顯的警告。 |
| `screen_capture` | 取得裝置畫面的即時影片（`host.screen.pipeTo`）。畫面上可能有訊息、密碼等任何內容，因此是獨立的權限。 |

- 註冊選單項目與按鈕不需要權限。
- 安裝時，使用者會看到 Addon 要求的權限，可逐項取消勾選。未授予權限的 API，呼叫時會得到 `PermissionDenied`，可用
  `try/catch` 接住並降級處理。程式會記錄 Addon 嘗試過哪些未授權的呼叫，設定頁的列表可以看到。
- `addon.json` 之後若改為要求更多權限，列表會顯示該 Addon 要求的權限比已允許的多，在使用者重新決定之前不會執行。
  要求變少則不受影響。

## TypeScript 型別

[../addon-api/](../addon-api/) 是 `host`、`console`、計時器與各種錯誤的型別定義（套件 `scrcpy-agent-addon-api`，主版本號對應
`addon.json` 的 `api`）。它不發佈到 npm，以 `.tgz` 形式提供，附在每個發行版中（`npm run addon-types` 會在 `out/` 產生，不需要
Docker）。安裝到 Addon 自己的專案（路徑為該檔案的存放位置）：

```
npm install --save-dev ./scrcpy-agent-addon-api-1.0.0.tgz
```

接著在該專案的設定檔（`tsconfig.json`，JavaScript 專案用 `jsconfig.json`）設定 `"lib": ["ES2022"]`，不加 `"dom"`（腳本的執行
環境沒有瀏覽器也沒有 Node，那些型別會描述不存在的功能），並設定 `"types": ["scrcpy-agent-addon-api"]`。全域的 `host`、
`console` 與計時器就有型別了。TypeScript 用打包器（例如 esbuild）編成單一的 `main.js`，放進 Addon 資料夾。

[example-addon/](example-addon/) 示範這個設定，也可直接複製為起點：它的 `package.json` 從自己的 `vendor/` 安裝一份 1.0.0 的
`.tgz`（`npm install` 即可），`jsconfig.json` 只寫套件名稱。範例固定在撰寫時的型別版本，不隨型別更新，也不納入任何檢查。
Addon 本身不需要 `package.json`，程式只讀 `addon.json` 與 `main.js`。

資料的型別（權限、每個事件帶什麼、各錯誤的 `code`）由程式本身的型別產生，因此與程式行為一致；函式的形狀則與程式提供的函式
清單核對。

## 範例

[example-addon/](example-addon/) 是可直接安裝的 Addon：

- 裝置按鈕「Say hello (example add-on)」，依設定（問候語、次數、樣式）記錄問候，並在有 `device_shell` 權限時對裝置執行
  `echo`；
- App 選單項目與工具列按鈕；
- 工具列按鈕「Analyze this screen (example add-on)」：啟動隨 Addon 附的 `analyzer.js`（以 `host.addon.folder()` 取得位置），
  並把畫面傳給它；它以 Node 執行，因此這台電腦上必須有 `node`；
- 一個計時器；
- 四個設定（`addon.json` 的 `settings`），其中 `verbose` 決定是否把全部八個事件記到控制台。

`main.js` 裡列出了幾個可以嘗試的情況。

## 安裝來測試

設定頁 →「Addon」→「加入」→「從資料夾」，選取 Addon 的資料夾。

- 安裝的是連結，不是複製：程式記住這個資料夾，每次開啟設定頁都重新讀取狀態。`main.js` 更動後，按該 Addon 的「重新載入」
  即可套用：從資料夾重新讀取並重新啟動，舊的註冊與計時器會被清除，使用者授予的權限不變（若 `addon.json` 要求了更多權限，
  列表會顯示它要求的權限比已允許的多，且在使用者重新決定之前不會執行）。
- 「禁用」可暫時停止 Addon 而不移除。Addon 會停止、它註冊的選單與按鈕全部收回，但安裝與使用者授予的權限保留；程式重新啟動後
  仍是禁用，禁用期間「重新載入」與重新決定權限都不會讓它執行。按「啟用」後重新執行 `main.js`。
- 移除只是忘記這個連結並停止 Addon，不會更動 Addon 的資料夾，也不會刪除 Addon 自己存的資料。
- 列表上每個 Addon 有狀態：可用、資料夾不見了、`addon.json` 不見了或壞了（原因會指出是哪個欄位）、id 變了、`main.js` 不見了、
  不支援這個系統或版本、要求的權限變多。可用的 Addon 還會顯示是執行中，還是沒有啟動（含原因）。
- 安裝、移除、重新決定權限、Addon 的記錄與被拒絕的呼叫，都在除錯控制台（側邊欄的「除錯控制台」）。
