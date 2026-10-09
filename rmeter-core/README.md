# rmeter-core

执行计划、用户上下文、采样接口和异步执行引擎。

## 断言接口

`assertion` 模块提供通用 `Assertion` 接口，以及 `AssertionInput` 和 `AssertionResult`。
组件接收采样结果、独立的完整响应体和用户上下文，返回带名称的断言诊断。
响应体不可用时可以通过 `unavailable_reason` 说明原因，避免对截断数据执行断言。

采样器在返回前通过 `SampleResult::add_assertion` 添加结果；失败会将采样 `success`
置为 false，通过不会覆盖已有失败。各项结果保存在 `SampleResult.assertions`，
引擎据最终 `success` 更新上下文和执行错误策略，IF/While 也读取该最终状态。
JSONPath 实现位于 `rmeter-component`，核心库不依赖 JSON 或 HTTP 实现。

## 控制器结构

结构参考 JMeter 的 [GenericController](https://github.com/apache/jmeter/blob/master/src/core/src/main/java/org/apache/jmeter/control/GenericController.java)、
[IfController](https://github.com/apache/jmeter/blob/master/src/core/src/main/java/org/apache/jmeter/control/IfController.java) 和
[WhileController](https://github.com/apache/jmeter/blob/master/src/core/src/main/java/org/apache/jmeter/control/WhileController.java)。
实现使用 Rust 组合，控制器定义共享，运行状态按用户隔离。

- `GenericController<S>`：统一名称和子节点定义，作为通用顺序控制器。
- `Controller<S>`：统一 `generic`、`should_enter`、`should_repeat`、`before_sample` 生命周期接口。
- `IfController<S>` / `WhileController<S>`：组合 GenericController，通过生命周期钩子决定进入、重检和重复。
- `GenericControllerState`：游标内部通用状态，管理子节点位置、当前控制器迭代次数、首次进入和完成状态。
- `ExecutionCursor`：显式栈遍历，不递归执行；所有节点共用遍历、重新初始化和结束处理。

`Node::Sequence` 和 `Node::Loop` 保留；新增 `Node::Controller`、`Node::If`、`Node::While`。
计划中的控制器不持有用户运行状态，因此可以由多个并发用户共享。

## 条件与控制器示例

```rust
use rmeter_core::{
    context::UserContext,
    controller::{CursorStep, ExecutionCursor, IfController, WhileController},
    plan::Node,
};

let nodes = vec![
    Node::If(IfController::new("登录检查", "${enabled}", vec![Node::Sample("if")])),
    Node::While(WhileController::new("轮询", "LAST", vec![Node::Sample("while")])),
];
let mut context = UserContext::new("用户组", 0);
context.variables.insert("enabled".into(), "true".into());
let mut cursor = ExecutionCursor::new(&nodes);
let mut executed = Vec::new();
loop {
    match cursor.step(&mut context).unwrap() {
        CursorStep::Sample(sample) => {
            executed.push(*sample);
            // The engine performs actual sampling, then updates the context.
            context.last_sample_ok = *sample != "while";
        }
        CursorStep::Yield => {
            // In async callers: tokio::task::yield_now().await;
        }
        CursorStep::Done => break,
    }
}
assert_eq!(executed, vec!["if", "while"]);
```

### IF

默认每次进入检查一次，成立后执行完整个循环体；不成立只跳过当前进入，父循环下次进入会重新检查。
`evaluate_all = true` 在每次获取采样器时重新检查，包含嵌套控制器中的采样器；变为 false 时放弃当前剩余子树。

`Condition::Expression`（也可直接传字符串）支持 `${variable}`，展开后只有忽略大小写的 `true` 成立。
不执行 JavaScript、Groovy 或 `${__function(...)}`。

### While

字符串条件使用 JMeter 的主要变量表达式语义：

- 空条件：无条件进入，在循环体结束时根据最后一个采样结果决定是否继续。
- `LAST`：进入前和循环体结束后都检查 `last_sample_ok`。
- 其他字符串：展开后等于忽略大小写的 `false` 才退出。
- 非空条件在每轮开始及结束检查；求值无副作用。
- 直接空子节点列表立即完成，不空转。

也可传入结构化 `Condition`，这时直接使用其布尔求值结果。
每个 While 将零起始循环索引写入该用户变量 `__jm__<控制器名称>__idx`，再次进入或重置后从 0 开始。
需要引用循环索引时，各个 While 应使用不同名称；变量保留最后写入值，不在退出时删除。

### 结构化条件

`Condition` 支持常量、严格布尔变量、上次采样结果、比较、Not、All、Any。All/Any 短路求值。
`Operand` 支持字符串常量、变量、线程组迭代次数、用户索引和上次采样结果。
Equal/NotEqual 比较字符串；大小比较使用精确 i128 整数，避免 u64 循环计数的浮点精度损失。

```rust
use rmeter_core::controller::{Condition, Operand, CompareOperator};
let condition = Condition::Compare {
    left: Operand::Variable("count".into()),
    operator: CompareOperator::Less,
    right: Operand::Literal("10".into()),
};
```

表达式保留 `iteration`、`user_index`、`last_sample_ok` 和 `JMeterThread.last_sample_ok` 为上下文字段名。
缺失变量、非法严格布尔值、整数转换错误和非法变量表达式返回明确错误。
这与 JMeter 对部分表达式错误的容错行为不同；引擎返回带用户组、用户索引及控制器名称的 `EngineError::ControllerFailed`，终止当前运行，不生成伪采样事件。

## 游标与引擎

游标由原先的 Iterator 接口改为 `step(&mut UserContext)`，每次推进读取最新上下文。
返回 `Sample`、`Yield` 或 `Done`，条件错误单独返回 Result 的 Err。
连续控制节点推进具有步数预算；调用方收到 Yield 后应让出异步调度，以保证无采样的无限循环能被取消或运行时限中断。

线程组中整轮 IF 被跳过也会消耗一次迭代，并继续后续迭代。
只有静态空执行树提前结束无限线程组。`NextIteration` 丢弃当前控制器栈，下次线程组迭代重新初始化。
`StopUser`、`StopTest`、运行时限以及事件接收端关闭都保留原有处理方式。

条件读取采样器对 UserContext 的修改。HTTP 响应字段若要作为条件变量，需要响应提取器或其他组件写入 variables。
