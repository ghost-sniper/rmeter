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
