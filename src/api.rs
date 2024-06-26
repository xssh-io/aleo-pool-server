use std::{convert::Infallible, net::SocketAddr, sync::Arc};

use serde_json::json;
use snarkvm::prelude::{Address, CanaryV0};
use tokio::task;
use tracing::info;
use warp::{
    addr::remote,
    get,
    head,
    path,
    reply,
    reply::{json, Json, WithStatus},
    serve,
    Filter,
    Reply,
};

use crate::{Accounting, Server};

/// 启动API服务器
///
/// 此函数创建并启动一个异步任务，该任务负责运行API服务器，提供关于当前轮次、统计信息和地址统计等的数据接口。
/// 使用了Warp库来定义和路由HTTP请求。
///
/// 参数:
/// - `port`: 服务器监听的端口号。
/// - `accounting`: 会计信息的共享所有权对象，用于访问和更新会计数据。
/// - `server`: 服务器的共享所有权对象，用于访问和更新服务器相关数据。
pub fn start(port: u16, accounting: Arc<Accounting>, server: Arc<Server>) {
    task::spawn(async move {
        // 定义路由：获取当前轮次的信息
        let current_round = path("current_round")
            .and(use_accounting(accounting.clone()))
            .then(current_round)
            .boxed();

        // 定义路由：获取矿池的统计信息
        let pool_stats = path("stats").and(use_server(server.clone())).then(pool_stats).boxed();

        // 定义路由：获取特定地址的统计信息
        let address_stats = path!("stats" / String)
            .and(use_server(server.clone()))
            .then(address_stats)
            .boxed();

        // 定义路由：管理员获取当前轮次的信息（可能需要远程验证）
        let admin_current_round = path!("admin" / "current_round")
            .and(remote())
            .and(use_accounting(accounting.clone()))
            .then(admin_current_round)
            .boxed();

        // 将所有路由合并为一个终结点
        let endpoints = current_round
            .or(address_stats)
            .or(pool_stats)
            .or(admin_current_round)
            .boxed();

        // 定义HTTP请求的处理逻辑
        let routes = get()
            .or(head())
            .unify()
            .and(endpoints)
            .with(warp::log("aleo_pool_server::api"));

        // 启动API服务器并绑定到指定的IP和端口
        info!("Starting API server on port {}", port);
        serve(routes).run(([0, 0, 0, 0], port)).await;
    });
}

/**
 * 创建一个过滤器，用于在每个HTTP请求中共享会计信息。
 *
 * 该函数使用`Arc<Accounting>`来确保会计信息可以在多个线程之间共享，并且可以安全地被多个请求同时访问。
 * 过滤器在每个请求处理过程中，都会返回一个会计信息的克隆版本，供请求处理代码使用。
 * 这样做的目的是为了确保会计信息的线程安全和请求之间的隔离。
 *
 * @param accounting 一个共享的会计信息对象，使用`Arc`封装以支持线程安全的共享。
 * @return 返回一个实现了`Filter` trait的匿名结构体，该结构体可以通过`Clone` trait进行克隆。
 *         过滤器的`Extract`参数表示成功处理请求后返回的值（在这个例子中是一个`Arc<Accounting>`的克隆），
 *         `Error`参数表示处理过程中可能发生的错误（在这个例子中，由于使用了`Infallible`，表示不会发生错误）。
 */
fn use_accounting(
    accounting: Arc<Accounting>,
) -> impl Filter<Extract = (Arc<Accounting>,), Error = Infallible> + Clone {
    // 使用`warp::any()`创建一个过滤器，该过滤器可以匹配任何请求。
    // 然后使用`map`函数将匹配到的请求处理函数包装起来。
    // 处理函数是一个闭包，它在每次请求时返回`accounting`的克隆。
    warp::any().map(move || accounting.clone())
}

/**
 * 创建一个过滤器，用于在Warp框架中共享服务器实例。
 *
 * 该函数返回一个Warp框架的过滤器，该过滤器可以在每个请求中提供对服务器实例的访问。
 * 通过使用`Arc`来共享所有权，确保了服务器对象可以在多个请求并发访问时安全使用。
 *
 * 参数:
 * `server` - 一个共享的服务器实例，使用`Arc`来管理其生命周期。
 *
 * 返回值:
 * 实现了`Filter` trait的匿名结构体，该结构体能够从请求中提取出服务器实例，并且能够安全地克隆。
 */
fn use_server(server: Arc<Server>) -> impl Filter<Extract = (Arc<Server>,), Error = Infallible> + Clone {
    // 使用`warp::any()`创建一个匹配任何请求的过滤器
    // 然后使用`map`将这个过滤器转换为返回服务器实例的函数
    // `move`闭包确保了对`server`的所有权转移，以便在闭包中使用
    warp::any().map(move || server.clone())
}

/**
 * 获取矿池状态统计信息。
 *
 * 本异步函数通过查询服务器对象，获取当前矿池的在线地址数、在线证明者数以及矿池的计算速度。
 * 这些信息对于监控矿池的运行状态以及性能评估是非常重要的。
 *
 * @param server 一个共享的服务器对象，用于异步访问矿池的状态信息。
 * @returns 包含矿池状态统计信息的JSON对象。
 */
async fn pool_stats(server: Arc<Server>) -> Json {
    // 构建一个JSON对象，包含矿池的在线地址数、在线证明者数和计算速度。
    json(&json!({
        "online_addresses": server.online_addresses().await,
        "online_provers": server.online_provers().await,
        "speed": server.pool_speed().await,
    }))
}

/// 异步获取指定地址的统计信息。
///
/// 此函数接收一个地址字符串和一个服务器的Arc实例，尝试解析地址并获取相关统计信息。
/// 如果地址解析成功，它将返回地址的速度和证明者数量。
/// 如果地址解析失败，它将返回一个包含错误信息的响应。
async fn address_stats(address: String, server: Arc<Server>) -> impl Reply {
    // 尝试将地址字符串解析为Address类型
    if let Ok(address) = address.parse::<Address<CanaryV0>>() {
        // 异步获取地址的速度
        let speed = server.address_speed(address).await;
        // 异步获取地址的证明者数量
        let prover_count = server.address_prover_count(address).await;

        // 构建并返回一个包含统计信息的JSON响应，状态码为200 OK
        Ok::<WithStatus<Json>, Infallible>(reply::with_status(
            json(&json!({
                "online_provers": prover_count,
                "speed": speed,
            })),
            warp::http::StatusCode::OK,
        ))
    } else {
        // 地址解析失败，构建并返回一个包含错误信息的JSON响应，状态码为400 BAD_REQUEST
        Ok::<WithStatus<Json>, Infallible>(reply::with_status(
            json(&json!({
                "error": "invalid address"
            })),
            warp::http::StatusCode::BAD_REQUEST,
        ))
    }
}

/**
 * 获取当前轮次的信息。
 *
 * 本异步函数通过查询会计（Accounting）对象来获取当前轮次的详细数据，包括轮次编号、当前轮次编号
 * 和该轮次的证明者列表。这些信息对于跟踪系统中的轮次进度和参与者非常重要。
 *
 * 参数:
 * `accounting` - 一个`Accounting`的共享所有权实例，用于查询当前轮次信息。
 *
 * 返回值:
 * 一个包含当前轮次详情的JSON对象。具体包括:
 * - `n`：总轮次数。
 * - `current_n`：当前轮次的编号。
 * - `provers`：当前轮次的证明者列表。
 */
async fn current_round(accounting: Arc<Accounting>) -> Json {
    // 异步调用`current_round`方法获取当前轮次数据
    let data = accounting.current_round().await;

    // 将获取到的当前轮次数据转换为JSON格式返回
    json(&json! ({
        "n": data["n"],
        "current_n": data["current_n"],
        "provers": data["provers"],
    }))
}

/// 异步函数：获取当前回合信息
///
/// 此函数用于根据提供的地址和会计信息，返回当前回合的详情。
/// 如果提供的地址是回环地址（localhost），则返回当前回合的统计信息。
/// 如果地址不是回环地址，则表示请求的方法不被允许。
///
/// 参数:
/// - `addr`: 可选的Socket地址，用于判断是否为回环地址。
/// - `accounting`: 会计信息的共享引用，用于获取当前回合的统计信息。
///
/// 返回值:
/// - 如果地址是回环地址，返回当前回合的统计信息，状态码为OK。
/// - 如果地址不是回环地址，返回"Method Not Allowed"错误信息，状态码为METHOD_NOT_ALLOWED。
async fn admin_current_round(addr: Option<SocketAddr>, accounting: Arc<Accounting>) -> impl Reply {
    // 解包地址选项，如果不存在则默认处理
    let addr = addr.unwrap();

    // 判断地址是否为回环地址
    if addr.ip().is_loopback() {
        // 如果是回环地址，获取当前回合的统计信息
        let pplns = accounting.current_round().await;
        // 返回当前回合的统计信息，状态码为OK
        Ok::<WithStatus<Json>, Infallible>(reply::with_status(json(&pplns), warp::http::StatusCode::OK))
    } else {
        // 如果不是回环地址，返回方法不被允许的错误信息
        Ok::<WithStatus<Json>, Infallible>(reply::with_status(
            json(&"Method Not Allowed"),
            warp::http::StatusCode::METHOD_NOT_ALLOWED,
        ))
    }
}
