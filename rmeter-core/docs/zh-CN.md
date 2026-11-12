# rmeter-core

[English](crate) · **简体中文** · [繁體中文](crate::translations::zh_tw) · [日本語](crate::translations::ja) · [Français](crate::translations::fr) · [Deutsch](crate::translations::de)

crate 首页和 API 参考默认使用英文。此使用指南提供六种语言；Rust 标识符及 API 参考注释保留英文。

## 概述

`rmeter-core` 定义与协议无关的执行计划、虚拟用户状态、采样与断言 trait、控制器和 Tokio 引擎。实现自定义采样器或构造强类型计划时使用本 crate；加载 JSON 脚本使用 `rmeter`，内置传输组件使用 `rmeter-component`。

## 快速开始

下面的独立示例遍历条件计划，无需网络。遍历时 `Node::Sample` 可持有任意值；通过 `Engine` 执行时，采样类型须实现 `Sampler`。

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

## 执行与错误处理

创建包含 [`ThreadGroup`](crate::plan::ThreadGroup) 的 [`TestPlan`](crate::plan::TestPlan)，在 Tokio 内调用 [`Engine::run`](crate::engine::Engine::run)，同时消费有界事件通道。每个用户拥有独立变量和控制器游标。用户索引、线程组迭代索引从零开始；变量在该用户后续采样和迭代间保留。

[`ErrorPolicy`](crate::plan::ErrorPolicy) 决定采样失败后继续、进入下次迭代、停止用户或停止测试。基础设施与条件求值错误返回 [`EngineError`](crate::engine::EngineError)。断言通过不能恢复已有失败。丢弃运行 future 会中止用户任务。

IF 表达式仅将忽略大小写的 `true` 视为真。While 字符串条件在忽略大小写等于 `false` 时停止；空字符串和 `LAST` 使用特殊的上次采样语义。直接使用 [`ExecutionCursor`](crate::controller::ExecutionCursor) 时，收到 `CursorStep::Yield` 应让出异步调度，确保无采样的无限循环可取消。

## API 导航

- [`plan`](crate::plan)：不可变计划和错误策略。
- [`sampler`](crate::sampler) / [`assertion`](crate::assertion)：扩展 trait 与结果。
- [`context`](crate::context)：用户变量和有界脚本日志。
- [`controller`](crate::controller)：顺序、IF、While 和遍历。
- [`engine`](crate::engine) / [`event`](crate::event)：并发执行与事件。

## 原生函数

RandomString、changeCase、dateTimeConvert、Random、RandomDate 由 Rust、JavaScript 的 `functions` 对象和 `${__函数(...)}` 表达式共用。日期模式采用 JDK 21 ISO/SMART 规则，地区和时区数据内置，运行时不需要 Java。默认时区和语言在运行开始时从系统读取。非法调用明确报错；可选变量名将结果写入当前用户，表达式失败时回滚其写入。JavaScript 源码不进行表达式替换。控制器求值接口现接收可变 UserContext，以支持函数写变量。
