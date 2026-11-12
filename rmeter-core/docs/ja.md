# rmeter-core

[English](crate) · [简体中文](crate::translations::zh_cn) · [繁體中文](crate::translations::zh_tw) · **日本語** · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate のホームと API リファレンスは既定で英語です。このガイドは 6 言語に対応し、Rust 識別子と API コメントは英語のままです。

## 概要

`rmeter-core` はプロトコルに依存しない計画、仮想ユーザー状態、サンプラーとアサーションの trait、コントローラー、Tokio エンジンを定義します。独自サンプラーや型付き計画にはこの crate、JSON 読み込みには `rmeter`、組み込み通信機能には `rmeter-component` を使用します。

## クイックスタート

以下の自己完結した例は、ネットワークなしで条件付き計画を走査します。走査だけなら `Node::Sample` は任意の値を保持できますが、`Engine` による実行には `Sampler` の実装が必要です。

```rust
use rmeter_core::{
    context::UserContext,
    controller::{CursorStep, ExecutionCursor, IfController},
    plan::Node,
};

let nodes = vec![Node::If(IfController::new(
    "Enabled", "${enabled}", vec![Node::Sample("health")],
))];
let mut user = UserContext::new("Users", 0);
user.variables.insert("enabled".into(), "true".into());
let mut cursor = ExecutionCursor::new(&nodes);
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Sample(&"health")));
assert!(matches!(cursor.step(&mut user).unwrap(), CursorStep::Done));
```

## 実行とエラー処理

[`ThreadGroup`](crate::plan::ThreadGroup) を含む [`TestPlan`](crate::plan::TestPlan) を作成し、Tokio 内で [`Engine::run`](crate::engine::Engine::run) を呼び出すと同時に有界イベントチャネルを受信します。各ユーザーは独立した変数とカーソルを持ちます。ユーザーとグループ反復のインデックスは 0 始まりで、変数は同じユーザーの後続サンプルと反復に引き継がれます。

[`ErrorPolicy`](crate::plan::ErrorPolicy) は失敗時の継続、次の反復、ユーザー停止、テスト停止を選択します。基盤処理や条件評価の失敗は [`EngineError`](crate::engine::EngineError) になります。アサーション成功で既存の失敗を取り消すことはできません。実行 future の破棄はユーザータスクを中止します。

IF 式は大文字小文字を無視した `true` のみを真とします。While の文字列条件は `false` で停止し、空文字列と `LAST` には前回サンプルを使う特別な意味があります。[`ExecutionCursor`](crate::controller::ExecutionCursor) を直接使う場合、`CursorStep::Yield` でランタイムに制御を返し、サンプルを含まない無限ループもキャンセルできるようにしてください。

## API 一覧

- [`plan`](crate::plan)：不変の計画とエラーポリシー。
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion)：拡張 trait と結果。
- [`context`](crate::context)：ユーザー変数とサイズ制限付きログ。
- [`controller`](crate::controller)：順次実行、IF、While、走査。
- [`engine`](crate::engine) / [`event`](crate::event)：並行実行とイベント。

## ネイティブ関数

RandomString、changeCase、dateTimeConvert、Random、RandomDate は Rust、JavaScript の `functions` オブジェクト、`${__function(...)}` 式で共有します。日付パターンは JDK 21 の ISO/SMART 規則に従い、ロケールとタイムゾーンのデータを同梱するため実行時に Java は不要です。既定値は実行開始時のシステム設定です。不正な呼び出しはエラーとなります。省略可能な変数名で現在のユーザーに結果を保存し、式が失敗すると書き込みを取り消します。JavaScript ソースは式展開しません。コントローラーの評価は変数を書き込むため可変 UserContext を受け取ります。

## コントローラーの反復ごとのタイムアウト

Loop と While は反復開始時に時間枠をリセットします。JSON の `timeout_ms` は非負の安全な整数（ミリ秒）で、省略または 0 は無制限です。null、小数、式は無効です。JSON version 1 を維持します。入れ子の子ノード、タイマー、プロセッサー、サンプラーを含み、結果チャネルの送信待ちは除外します。期限切れ時は実行中のサンプルをキャンセルし、コントローラー全体を終了して、コントローラー名で `ControllerTimeout` の失敗を1件記録します。無名の Loop は「Loop 循环」です。完了したサンプルと変数は保持し、中断したサンプルは二重計上しません。最も早い期限を優先し、同時なら外側を優先します。`last_sample_ok` を false にし、スレッドグループのエラーポリシーを適用します。Continue はコントローラーの後から続行します。ユーザーのキャンセルとグループ期限の動作は変わりません。

Rust: `Node::LoopController(LoopController::new(name, count, children))` と `WhileController::timeout` の既定値は `Duration::ZERO` です。従来の `Node::Loop` は無制限のままです。`ComponentNode::LoopController` は name/count/timeout/children を持ち、`ComponentNode::While` にも timeout が必要です。カーソルを直接使う場合は `CursorStep::Timeout` を処理し、`timeout_remaining()` でサンプルの future を制限し、`exclude_reporting_time()` で結果送信待ちを除外します。
