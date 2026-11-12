# rmeter-component

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · **日本語** · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate のホームと API リファレンスは既定で英語です。このガイドは 6 言語に対応し、Rust 識別子と API コメントは英語のままです。

## 概要

`rmeter-component` は `rmeter-core` 向けの HTTP/HTTPS、PostgreSQL（JDBC 風の命名）、同期 JavaScript サンプラーを提供します。JSONPath・正規表現・XML アサーション、HTTP 抽出器、スコープ付き既定値、定数・ガウスタイマーも含みます。PostgreSQL は Rust ネイティブドライバーを使い、JVM や Java JDBC ドライバーは読み込みません。

## クイックスタート

Tokio ランタイム内で、`{"ready":true}` を返すローカルサービスに対して実行します。コンストラクターは静的設定を検証し、`sample` を await した時点で初めて HTTP リクエストを送信します。

```rust,no_run
use rmeter_component::{HttpRequestConfig, HttpSampler, JsonAssertionConfig};
use rmeter_core::{context::UserContext, sampler::Sampler};

# async fn example() -> Result<(), Box<dyn std::error::Error>> {
let mut config = HttpRequestConfig::with_url(
    "Health", "http://127.0.0.1:8080/health",
);
config.add_assertion(JsonAssertionConfig::equals(
    "Ready", "$.ready", serde_json::json!(true),
));
let sampler = HttpSampler::new(config)?;
let mut user = UserContext::new("Users", 0);
let result = sampler.sample(&mut user).await
    .map_err(|error| format!("{error:?}"))?;
assert!(result.success, "{:?}", result.response_message);
# Ok(())
# }
```

## 実行とエラー処理

[`ComponentCompiler`](crate::ComponentCompiler) はスコープ付き既定値、名前付き DB プール、タイマーを実行ノードへ変換します。内側の既定値は外側を上書きし、リクエスト設定は継承値を上書きします。明示した header/query リストは継承リスト全体を置き換えます。互換性のある HTTP クライアントと名前付き DB プールは共有しますが、ユーザー変数は分離されます。Cookie は自動保存しません。

順序は前処理 → タイマー → サンプラー → 後処理 → アサーションです。処理器は独立したサンプルイベントを生成せず、その失敗やアサーション失敗は親サンプルを失敗にします。JavaScript の変数変更は成功時のみ確定し、ロールバックは外部 DB 操作を取り消しません。後処理は読み取り専用 `prev` でアサーション前の結果を参照します。

HTTP の保存本文、アサーション入力、後処理入力は別々に制限されます。切り詰めた保存本文は完全なアサーション入力として扱いません。HTTP 実行エラーは失敗した `SampleResult` なので、`sample` が `Ok` でも `success` を確認してください。

## API 一覧

- [`http`](crate::http)：リクエスト、本文、既定値、HTTP ツリー。
- [`jdbc`](crate::jdbc)：名前付き PostgreSQL プールとパラメーター化クエリ。
- [`javascript`](crate::javascript)：スクリプト、制限、worker 設定。
- [`assertion`](crate::assertion) / [`extractor`](crate::extractor)：応答検査と変数抽出。
- [`component`](crate::component) / [`timer`](crate::timer)：混合ツリーと遅延。

## JavaScript と抽出器の worker

このワークスペースで worker をビルドし、同じバージョンの実行ファイルをホストアプリの隣へ配置します。または計画のコンパイル前に `rmeter_component::javascript::set_worker_executable` で信頼できる絶対パスを設定します（統合入口では `rmeter::component` 経由）。ホスト側の `RMETER_JS_WORKER` 設定も利用できます。JavaScript、処理器、ネイティブ HTTP 抽出器には worker が必要です。これらを使わない HTTP/JDBC 計画には不要です。worker は資源と障害を分離しますが、完全な OS サンドボックスではありません。

```sh
cargo build -p rmeter-component --release --bin rmeter-js-worker
```

## ネイティブ関数

RandomString、changeCase、dateTimeConvert、Random、RandomDate は Rust、JavaScript の `functions` オブジェクト、`${__function(...)}` 式で共有します。日付パターンは JDK 21 の ISO/SMART 規則に従い、ロケールとタイムゾーンのデータを同梱するため実行時に Java は不要です。既定値は実行開始時のシステム設定です。不正な呼び出しはエラーとなります。省略可能な変数名で現在のユーザーに結果を保存し、式が失敗すると書き込みを取り消します。JavaScript ソースは式展開しません。コントローラーの評価は変数を書き込むため可変 UserContext を受け取ります。

## コントローラーの反復ごとのタイムアウト

Loop と While は反復開始時に時間枠をリセットします。JSON の `timeout_ms` は非負の安全な整数（ミリ秒）で、省略または 0 は無制限です。null、小数、式は無効です。JSON version 1 を維持します。入れ子の子ノード、タイマー、プロセッサー、サンプラーを含み、結果チャネルの送信待ちは除外します。期限切れ時は実行中のサンプルをキャンセルし、コントローラー全体を終了して、コントローラー名で `ControllerTimeout` の失敗を1件記録します。無名の Loop は「Loop 循环」です。完了したサンプルと変数は保持し、中断したサンプルは二重計上しません。最も早い期限を優先し、同時なら外側を優先します。`last_sample_ok` を false にし、スレッドグループのエラーポリシーを適用します。Continue はコントローラーの後から続行します。ユーザーのキャンセルとグループ期限の動作は変わりません。

Rust: `Node::LoopController(LoopController::new(name, count, children))` と `WhileController::timeout` の既定値は `Duration::ZERO` です。従来の `Node::Loop` は無制限のままです。`ComponentNode::LoopController` は name/count/timeout/children を持ち、`ComponentNode::While` にも timeout が必要です。カーソルを直接使う場合は `CursorStep::Timeout` を処理し、`timeout_remaining()` でサンプルの future を制限し、`exclude_reporting_time()` で結果送信待ちを除外します。
