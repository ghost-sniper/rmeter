# rmeter

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · **日本語** · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate のホームと API リファレンスは既定で英語です。このガイドは 6 言語に対応し、Rust 識別子と API コメントは英語のままです。

## 概要

`rmeter` は Rust/Tokio による負荷テスト用ワークスペースのスクリプト実行窓口です。バージョン 1 の JSON 計画を解析し、HTTP、PostgreSQL、JavaScript コンポーネントをコンパイルして実行イベントを送信します。既定ではライブラリをビルドし、任意の `cli` feature でコマンドライン実行を有効にします。

## クイックスタート

既存の Tokio ランタイム内で実行し、例の URL で HTTP サービスを起動してください。イベントは並行して受信する必要があります。有界チャネルが満杯になるとエンジンは待機します。Cargo のローカルパス依存関係として `rmeter` を追加し、この例には `tokio` と `serde_json` も追加します。

```rust,no_run
use rmeter::{report::{ReportEvent, RunSummary}, run_script};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let source = r#"{
  "version": 1,
  "name": "Smoke test",
  "groups": [{
    "name": "Users",
    "users": 2,
    "iterations": 1,
    "children": [{
      "type": "http_request",
      "name": "Health",
      "url": "http://127.0.0.1:8080/health"
    }]
  }]
}"#;
let (sender, mut receiver) = tokio::sync::mpsc::channel(128);
let consume = async move {
    let mut summary = RunSummary::default();
    while let Some(event) = receiver.recv().await {
        summary.observe(&event);
        let report = ReportEvent::from(event);
        println!("{}", serde_json::to_string(&report).unwrap());
    }
    summary
};
let (result, summary) = tokio::join!(run_script(source, sender), consume);
result?;
assert_eq!(summary.failed, 0);
# Ok(())
# }
```

## 実行とエラー処理

[`parse_script`](crate::parse_script) は HTTP リクエストや PostgreSQL 接続を行わずに検証とコンパイルを実施します。ただし JavaScript とネイティブ HTTP 抽出器は検証時にも別プロセスの worker を必要とします。BOM 付き UTF-8 JSON を受け付け、未知のフィールドや未対応バージョンは拒否します。変数は文字列で、仮想ユーザーごとに分離されます。

[`run_script`](crate::run_script) と [`run_script_file`](crate::run_script_file) は呼び出し側のランタイムを利用し、実行中の future を破棄すると仮想ユーザーをキャンセルします。サンプル失敗はイベントで報告され、エンジンが `Ok(())` を返す場合もあります。`success`、[`RunSummary`](crate::report::RunSummary)、戻り値を併せて確認してください。`Finished` だけでは成功を保証しません。インデックスは 0 始まり、シリアライズされた時刻は Unix ミリ秒です。

## API 一覧

- [`script`](crate::script)：解析、ファイル読み込み、[`ScriptError`](crate::ScriptError)。
- [`report`](crate::report)：シリアライズ可能なイベントとサンプル集計。
- [`core`](crate::core)：計画、コントローラー、コンテキスト、エンジン。
- [`component`](crate::component)：HTTP/JDBC/JavaScript、アサーション、抽出器、タイマー。

## JavaScript と抽出器の worker

このワークスペースで worker をビルドし、同じバージョンの実行ファイルをホストアプリの隣へ配置します。または計画のコンパイル前に `rmeter_component::javascript::set_worker_executable` で信頼できる絶対パスを設定します（統合入口では `rmeter::component` 経由）。ホスト側の `RMETER_JS_WORKER` 設定も利用できます。JavaScript、処理器、ネイティブ HTTP 抽出器には worker が必要です。これらを使わない HTTP/JDBC 計画には不要です。worker は資源と障害を分離しますが、完全な OS サンドボックスではありません。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## CLI とドキュメント

```sh
cargo run -p rmeter --features cli -- examples/http.json --check
cargo run -p rmeter --features cli -- examples/http.json
cargo doc --workspace --no-deps --open
cargo test --workspace --doc
```

## ネイティブ関数

RandomString、changeCase、dateTimeConvert、Random、RandomDate は Rust、JavaScript の `functions` オブジェクト、`${__function(...)}` 式で共有します。日付パターンは JDK 21 の ISO/SMART 規則に従い、ロケールとタイムゾーンのデータを同梱するため実行時に Java は不要です。既定値は実行開始時のシステム設定です。不正な呼び出しはエラーとなります。省略可能な変数名で現在のユーザーに結果を保存し、式が失敗すると書き込みを取り消します。JavaScript ソースは式展開しません。コントローラーの評価は変数を書き込むため可変 UserContext を受け取ります。

## コントローラーの反復ごとのタイムアウト

Loop と While は反復開始時に時間枠をリセットします。JSON の `timeout_ms` は非負の安全な整数（ミリ秒）で、省略または 0 は無制限です。null、小数、式は無効です。JSON version 1 を維持します。入れ子の子ノード、タイマー、プロセッサー、サンプラーを含み、結果チャネルの送信待ちは除外します。期限切れ時は実行中のサンプルをキャンセルし、コントローラー全体を終了して、コントローラー名で `ControllerTimeout` の失敗を1件記録します。無名の Loop は「Loop 循环」です。完了したサンプルと変数は保持し、中断したサンプルは二重計上しません。最も早い期限を優先し、同時なら外側を優先します。`last_sample_ok` を false にし、スレッドグループのエラーポリシーを適用します。Continue はコントローラーの後から続行します。ユーザーのキャンセルとグループ期限の動作は変わりません。

Rust: `Node::LoopController(LoopController::new(name, count, children))` と `WhileController::timeout` の既定値は `Duration::ZERO` です。従来の `Node::Loop` は無制限のままです。`ComponentNode::LoopController` は name/count/timeout/children を持ち、`ComponentNode::While` にも timeout が必要です。カーソルを直接使う場合は `CursorStep::Timeout` を処理し、`timeout_remaining()` でサンプルの future を制限し、`exclude_reporting_time()` で結果送信待ちを除外します。
