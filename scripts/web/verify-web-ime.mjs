// TASK-8.5-S22（REQ-8 / Issue #203、親 #181）: Chrome DevTools Protocol
// （Runtime.evaluate）を Node 標準の WebSocket クライアント（Node 24 組み込み）のみで
// 叩き、headless chromium 上で日本語入力（IME composition）シナリオを駆動して
// `crates/fandhe-web-dom/src/mount.rs` の `attach_input` composition 配線を
// end-to-end に検証する。npm 依存を持ち込まない方針は dump-ax-tree.mjs
// （TASK-4.4-S4）と同じ（security.md「A06 脆弱な依存」の最小依存方針）。
//
// 使い方: node verify-web-ime.mjs <cdp-host:port> <検証対象 URL> <結果 JSON 出力パス>
//
// # CDP IME シミュレーションではなく合成イベント dispatch を使う理由
//
// CDP の `Input.imeSetComposition` は headless 環境・単純な `<input>` 要素に対して
// 確実に compositionstart/update/end イベント列を発火させる保証がない
// （`s22-web-japanese-input.md` §4 参照）。そのため本スクリプトは
// `Runtime.evaluate` でページ内に固定スクリプトを実行させ、`CompositionEvent`/
// `InputEvent` を `dispatchEvent` で合成発火させる方式を採る。これは
// 実ブラウザの「ネイティブ IME が `<input>.value` を先行更新してから
// compositionend を発火する」挙動を、値の書き込み（テストスクリプト側）+
// イベント発火（`dispatchEvent`）の 2 手順に分解して再現するものであり、
// `attach_input` の実配線（DOM リスナー登録・`is_composing` スキップ・
// `compositionend` commit・全再構築後のフォーカス復元）を実際に通す点で、
// native シーケンス再現テスト（層 B、`fandhe_core`/`fandhe_web_dom::ime` を直接
// 呼ぶだけで DOM を経由しない）より高い保証を持つ。
//
// このスクリプトは検証対象ページの内容を読み取って実行するのではなく、
// こちらが用意した固定シナリオスクリプトのみを評価する（A03 対策の考え方は
// dump-ax-tree.mjs と同じ）。

const [, , cdpAddress, targetUrl, outPath] = process.argv;

if (!cdpAddress || !targetUrl || !outPath) {
  console.error(
    "使い方: node verify-web-ime.mjs <cdp-host:port> <検証対象 URL> <結果 JSON 出力パス>",
  );
  process.exit(1);
}

async function findPageTarget(cdpAddress) {
  const listRes = await fetch(`http://${cdpAddress}/json/list`);
  if (!listRes.ok) {
    throw new Error(`/json/list の取得に失敗した: HTTP ${listRes.status}`);
  }
  const targets = await listRes.json();
  const page = targets.find((t) => t.type === "page");
  if (!page) {
    throw new Error("page 種別の CDP ターゲットが見つからない");
  }
  return page;
}

function connect(webSocketDebuggerUrl) {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(webSocketDebuggerUrl);
    ws.addEventListener("open", () => resolve(ws));
    ws.addEventListener("error", (err) => reject(err));
  });
}

function makeRpc(ws) {
  let nextId = 0;
  const pending = new Map();
  const rejectAllPending = (reason) => {
    for (const { reject } of pending.values()) {
      reject(reason);
    }
    pending.clear();
  };
  ws.addEventListener("message", (event) => {
    const msg = JSON.parse(event.data);
    if (msg.id !== undefined && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id);
      pending.delete(msg.id);
      if (msg.error) {
        reject(new Error(`CDP エラー: ${JSON.stringify(msg.error)}`));
      } else {
        resolve(msg.result);
      }
    }
  });
  ws.addEventListener("error", (err) => {
    rejectAllPending(new Error(`CDP WebSocket エラー: ${err.message ?? err}`));
  });
  ws.addEventListener("close", () => {
    rejectAllPending(new Error("CDP WebSocket が切断された（chromium クラッシュの可能性）"));
  });
  return function call(method, params = {}) {
    const id = ++nextId;
    return new Promise((resolve, reject) => {
      pending.set(id, { resolve, reject });
      ws.send(JSON.stringify({ id, method, params }));
    });
  };
}

/** `Runtime.evaluate` で expression を実行し、戻り値（JSON シリアライズ可能な値）を
 * 得る。ページ側で例外が投げられた場合は Error として reject する。 */
async function evaluate(call, expression) {
  const result = await call("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
  });
  if (result.exceptionDetails) {
    const text =
      result.exceptionDetails.exception?.description ??
      JSON.stringify(result.exceptionDetails);
    throw new Error(`ページ側で例外: ${text}`);
  }
  return result.result?.value;
}

/** wasm のロード・`mount_sample_screen` によるマウント完了を待つ（固定 sleep では
 * なく、期待する初期状態（input が存在し value === "下書き"）が現れるまで
 * ポーリングする、`dump-ax-tree.mjs` の `waitForExpectedRoles` と同じ作法）。 */
async function waitForMount(call, maxAttempts) {
  for (let attempt = 0; attempt < maxAttempts; attempt += 1) {
    const ready = await evaluate(
      call,
      `(() => {
        const input = document.querySelector("#fandhe-dom-root input");
        return !!input && input.value === "下書き";
      })()`,
    );
    if (ready) {
      return;
    }
    await new Promise((r) => setTimeout(r, 500));
  }
  throw new Error("wasm マウント完了（input.value === \"下書き\"）待ちがタイムアウトした");
}

/** シナリオ共通の合成イベント発火ヘルパーをページ側へ注入する。DOM は
 * `mount.rs` の全再構築方式のため、compositionend 等で input 要素が丸ごと
 * 差し替わる。そのため各ステップで `document.querySelector` を都度呼び直し、
 * 古い要素への参照を使い回さない（`mount.rs` の実配線と同じ前提）。 */
const SCENARIO_HARNESS = `
function currentInput() {
  const input = document.querySelector("#fandhe-dom-root input");
  if (!input) {
    throw new Error("#fandhe-dom-root input が見つからない");
  }
  return input;
}

function dispatchComposition(type, data) {
  const input = currentInput();
  input.dispatchEvent(new CompositionEvent(type, { data, bubbles: true, composed: true }));
}

function dispatchInput(isComposing) {
  const input = currentInput();
  input.dispatchEvent(
    new InputEvent("input", { bubbles: true, composed: true, isComposing }),
  );
}

// mount.rs は commit のたびに全再構築（既存 <input> を除去し document.createElement で
// 新しい要素を作り直す）を行うため、既存要素へ付与した dataset 属性は commit を境に
// 必ず消える。逆に言えば、attach_input のリスナーが実際に発火せず commit 経路が
// 一切走らない「完全に不活性」なケースでは、要素が使い回されるためマーカーが残り続ける。
// テストスクリプト自身による value 書き込みの読み返しだけでは区別できない
// 「実際に配線が動いたか」を、このマーカーの消失有無で検証する。
function markCurrentInput(label) {
  currentInput().dataset.qaMarker = label;
}

function currentMarkerLabel() {
  return currentInput().dataset.qaMarker ?? null;
}
`;

/** シナリオ 1: 標準的な日本語入力（かんじ → 漢字）。
 *
 * ネイティブ IME は preedit・確定のいずれでも `<input>.value` を先行更新してから
 * composition イベントを発火するため、このテストスクリプトも同じ順序
 * （value 書き込み → dispatch）を踏む。`compositionend` 到達時に
 * `attach_input` の commit 経路（`input.value()` 読み取り → `input_handlers` 呼び出し
 * → 全再構築）が走り、再構築後の新しい `<input>` 要素の `value` が確定値になる
 * ことを確認する。
 *
 * `value` の読み返しだけではテストスクリプト自身が書いた値を読み返しているに
 * すぎず、`attach_input` のリスナーが実際に発火したかを区別できない
 * （§4.2 レビュー指摘）。`compositionend` 発火直前にマーカーを付与し、`mount.rs`
 * の全再構築（要素差し替え）でマーカーが消えていることを合わせて確認することで、
 * commit 経路が実際に動作したことを検証する。`focused` も同じ理由で、マーカー
 * 消失（＝要素が実際に差し替わったこと）を前提にして初めて `restore_focus_state`
 * によるフォーカス復元ロジックの有無を検証できる。
 */
async function scenarioStandardInput(call) {
  await evaluate(
    call,
    `${SCENARIO_HARNESS}
    (() => {
      const input = currentInput();
      input.focus();
      dispatchComposition("compositionstart", "");
      const steps = [
        { value: "下書きか", data: "か" },
        { value: "下書きかん", data: "かん" },
        { value: "下書き漢字", data: "漢字" },
      ];
      for (const step of steps) {
        currentInput().value = step.value;
        dispatchComposition("compositionupdate", step.data);
      }
      currentInput().value = "下書き漢字";
      markCurrentInput("before-compositionend");
      dispatchComposition("compositionend", "漢字");
    })();
    "done"`,
  );

  const value = await evaluate(call, `currentInput().value`);
  const focused = await evaluate(
    call,
    `document.activeElement === currentInput()`,
  );
  const rebuilt = await evaluate(
    call,
    `currentMarkerLabel() !== "before-compositionend"`,
  );
  return {
    name: "standard_input",
    expected: { value: "下書き漢字", focused: true, rebuilt: true },
    actual: { value, focused, rebuilt },
    pass: value === "下書き漢字" && focused === true && rebuilt === true,
  };
}

/** シナリオ 2: 変換キャンセル（Esc 相当）。
 *
 * ブラウザは変換キャンセル時に preedit を取り消して `<input>.value` を元の値へ
 * 戻してから `compositionend`（`data` は空文字列になりうる）を発火する
 * （`s21-web-composition-ime.md` §2 事実 #3）。この挙動を再現し、
 * `mount.rs` の commit 経路を経ても値が変化しないことを確認する。
 *
 * `data` が空文字列でも `compositionend` は必ず commit 経路（全再構築）を通る
 * （`attach_input` は `End` を無条件で `Commit` に変換し `input.value()` を素通しする、
 * `mount.rs` 参照）ため、シナリオ 1 と同じくマーカー消失で経路が実際に動いたことを
 * 確認する（マーカーが残ったままなら、値が変化していないのは単に配線が不活性
 * だったからという可能性を排除できない）。
 */
async function scenarioCancelledComposition(call) {
  await evaluate(
    call,
    `${SCENARIO_HARNESS}
    (() => {
      const input = currentInput();
      input.focus();
      dispatchComposition("compositionstart", "");
      currentInput().value = "下書きに";
      dispatchComposition("compositionupdate", "に");
      // ブラウザがキャンセルにより preedit を取り消し、元の値へ戻す。
      currentInput().value = "下書き漢字";
      markCurrentInput("before-compositionend");
      dispatchComposition("compositionend", "");
    })();
    "done"`,
  );

  const value = await evaluate(call, `currentInput().value`);
  const rebuilt = await evaluate(
    call,
    `currentMarkerLabel() !== "before-compositionend"`,
  );
  return {
    name: "cancelled_composition",
    expected: { value: "下書き漢字", rebuilt: true },
    actual: { value, rebuilt },
    pass: value === "下書き漢字" && rebuilt === true,
  };
}

/** シナリオ 3: `compositionend` 後に後続 `input`（`isComposing:false`）が発火しても
 * 二重挿入されないこと（S21 §5 チェックリスト項目 2 の自動化）。
 *
 * `mount.rs` の `attach_input` は `input` イベントを `is_composing` で判定して
 * スキップするのは compositionend **前**のみで、`compositionend` 後に届く
 * `isComposing:false` の `input` は通常の commit 経路を通る。ただしどちらも
 * `input.value()`（全文）を読み直すだけの配線のため、同一値であれば再代入・
 * 再描画のみで buffer が重複しないことを確認する。
 *
 * `compositionend` の commit と、後続の `isComposing:false` `input` の commit は
 * それぞれ独立に全再構築を引き起こす（`attach_input` の `commit` クロージャは
 * どちらの経路からも呼ばれる、`mount.rs` 参照）。2 回とも実際に再構築が起きた
 * ことをマーカー消失で確認したうえで、最終的な値が二重挿入されていないことを
 * 確認する（マーカーを見ない場合、配線が完全に不活性でも「値が変わらない」の
 * 一点だけで pass してしまう、§4.2 レビュー指摘）。
 */
async function scenarioNoDoubleCommitAfterTrailingInput(call) {
  await evaluate(
    call,
    `${SCENARIO_HARNESS}
    (() => {
      const input = currentInput();
      input.focus();
      dispatchComposition("compositionstart", "");
      currentInput().value = "下書き猫";
      dispatchComposition("compositionupdate", "猫");
      currentInput().value = "下書き猫";
      markCurrentInput("before-compositionend");
      dispatchComposition("compositionend", "猫");
    })();
    "done"`,
  );
  const rebuiltAfterCompositionEnd = await evaluate(
    call,
    `currentMarkerLabel() !== "before-compositionend"`,
  );

  await evaluate(
    call,
    `${SCENARIO_HARNESS}
    (() => {
      markCurrentInput("before-trailing-input");
      // Chromium/Safari で観測されうる、compositionend 直後の isComposing:false input。
      dispatchInput(false);
    })();
    "done"`,
  );
  const rebuiltAfterTrailingInput = await evaluate(
    call,
    `currentMarkerLabel() !== "before-trailing-input"`,
  );

  const value = await evaluate(call, `currentInput().value`);
  return {
    name: "no_double_commit_after_trailing_input",
    expected: {
      value: "下書き猫",
      rebuiltAfterCompositionEnd: true,
      rebuiltAfterTrailingInput: true,
    },
    actual: { value, rebuiltAfterCompositionEnd, rebuiltAfterTrailingInput },
    pass:
      value === "下書き猫" &&
      rebuiltAfterCompositionEnd === true &&
      rebuiltAfterTrailingInput === true,
  };
}

async function main() {
  const page = await findPageTarget(cdpAddress);
  const ws = await connect(page.webSocketDebuggerUrl);
  const call = makeRpc(ws);

  await call("Page.navigate", { url: targetUrl });
  await call("Runtime.enable");

  await waitForMount(call, 20);

  const scenarios = [
    await scenarioStandardInput(call),
    await scenarioCancelledComposition(call),
    await scenarioNoDoubleCommitAfterTrailingInput(call),
  ];

  const allPass = scenarios.every((s) => s.pass);

  const { writeFileSync } = await import("node:fs");
  writeFileSync(
    outPath,
    JSON.stringify(
      {
        url: targetUrl,
        capturedAtUtc: new Date().toISOString(),
        allPass,
        scenarios,
      },
      null,
      2,
    ),
  );

  ws.close();

  if (!allPass) {
    console.error(
      `[verify-web-ime] エラー: 一部シナリオが期待値と不一致だった。結果は ${outPath} に保存済み。`,
    );
    process.exit(1);
  }

  console.log(`[verify-web-ime] すべてのシナリオが期待値と一致: ${outPath}`);
}

main().catch((err) => {
  console.error(`[verify-web-ime] エラー: ${err.message ?? err}`);
  process.exit(1);
});
