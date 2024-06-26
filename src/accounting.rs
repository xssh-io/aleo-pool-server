use std::{
    collections::{HashMap, VecDeque},
    fs::create_dir_all,
    path::PathBuf,
    sync::{atomic::AtomicBool, Arc},
    time::{Duration, Instant},
};

use anyhow::{anyhow, Error, Result};
use cache::Cache;
use dirs::home_dir;
use parking_lot::RwLock;
use savefile::{load_file, save_file};
use savefile_derive::Savefile;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use snarkvm::{ledger::puzzle::PuzzleSolutions, prelude::CanaryV0};
use tokio::{
    sync::{
        mpsc::{channel, Sender},
        RwLock as TokioRwLock,
    },
    task,
    time::sleep,
};
#[allow(unused_imports)]
use tracing::{debug, error, info};

#[cfg(feature = "db")]
use crate::db::DB;
use crate::{
    accounting::AccountingMessage::{NewShare, NewSolution},
    AccountingMessage::{Exit, SetN},
};

/// `PayoutModel` 是一个trait，定义了某种收益分配模型应具备的添加份额的方法。
///
/// 任何实现了这个trait的类型都必须提供一个方法来添加新的份额。
/// 这个trait的目的是为了在不同的收益分配策略之间提供一种抽象，使得可以在不关心具体实现的情况下处理收益分配。
trait PayoutModel {
    /// `add_share` 方法用于向分配模型中添加一个新的份额。
    ///
    /// # 参数
    /// `share` - 要添加的份额。
    ///
    /// # 方法作用
    /// 该方法的实现应该能够将传入的份额`share`整合到当前的分配模型中。
    /// 具体的整合方式取决于实现的具体逻辑，可以是累加，也可以是根据某些规则进行分配等。
    fn add_share(&mut self, share: Share);
}

/// `Share` 结构体代表了一种可转让的资产份额。
#[derive(Clone, Savefile)]
struct Share {
    /// 份额的价值，以u64类型表示，单位可以是自定义的。
    value: u64,
    /// 份额当前的所有者，使用String类型来存储所有者的名称或标识。
    owner: String,
}

/**
 * Share结构体的实现
 *
 * 该实现提供了初始化Share对象的方法。
 */
impl Share {

    /**
     * 初始化Share结构体
     *
     * 该方法用于创建一个新的Share对象，初始化其value和owner字段。
     *
     * @param value 股票的价值，以u64类型表示。
     * @param owner 股票的所有者，以String类型表示。
     * @return 返回一个新的Share对象，其value和owner字段已被初始化。
     */
    pub fn init(value: u64, owner: String) -> Self {
        Share { value, owner }
    }
}

/// PPLNS（Pay Per Last N Shares）是一种矿工收益分配算法的实现。
/// 它维护了一个队列来存储矿工的分享（Share），以及两个计数器来跟踪当前和累计的分享数。
#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Savefile)]
struct PPLNS {
    queue: VecDeque<Share>,
    current_n: Arc<RwLock<u64>>,
    n: Arc<RwLock<u64>>,
}

impl PPLNS {

    /// 从磁盘加载PPLNS状态。
    /// 如果家目录不存在，则抛出panic。
    /// 如果状态文件不存在，则初始化一个新的PPLNS状态。
    pub fn load() -> Self {
        let home = home_dir();
        if home.is_none() {
            panic!("No home directory found");
        }
        create_dir_all(home.as_ref().unwrap().join(".aleo_pool_TestnetV0_2")).unwrap();
        let db_path = home.unwrap().join(".aleo_pool_TestnetV0_2/state");
        if !db_path.exists() {
            return PPLNS {
                queue: VecDeque::new(),
                current_n: Default::default(),
                n: Default::default(),
            };
        }
        load_file::<PPLNS, PathBuf>(db_path, 0).unwrap()
    }

    /// 将PPLNS状态保存到磁盘。
    /// 如果家目录不存在，则抛出panic。
    pub fn save(&self) -> std::result::Result<(), Error> {
        let home = home_dir();
        if home.is_none() {
            panic!("No home directory found");
        }
        let db_path = home.unwrap().join(".aleo_pool_TestnetV0_2/state");
        save_file(db_path, 0, self).map_err(|e| anyhow!("Failed to save PPLNS state: {}", e))
    }

    /// 更新PPLNS的累计分享数n，并根据新的n调整当前分享数current_n。
    /// 如果新的n小于当前累计分享数，队列中的过期分享将被移除。
    pub fn set_n(&mut self, n: u64) {
        let start = Instant::now();
        let mut current_n = self.current_n.write();
        let mut self_n = self.n.write();
        if n < *self_n {
            while *current_n > n {
                let share = self.queue.pop_front().unwrap();
                *current_n -= share.value;
            }
        }
        *self_n = n;
        debug!("set_n took {} us", start.elapsed().as_micros());
    }
}

/// 实现PayoutModel trait以定义PPLNS（Pay Per Last N Shares）支付模型的行为。
impl PayoutModel for PPLNS {

    /// 添加一个矿工的分享到处理队列中。
    ///
    /// 此方法确保了分享值的累加和队列的管理遵循PPLNS的规则。特别是，它确保了
    /// 当当前累计分享值超过预设值时，会从队列前端移除分享，以维护队列的大小在合理范围内。
    fn add_share(&mut self, share: Share) {
        // 记录开始时间以评估此操作的性能。
        let start = Instant::now();

        // 将新的分享添加到处理队列的尾部。
        self.queue.push_back(share.clone());

        // 获取对current_n的可写锁，用于更新当前的分享累计值。
        let mut current_n = self.current_n.write();

        // 读取n的值，用于比较和可能的队列收缩。
        let self_n = self.n.read();

        // 累加当前分享值。
        *current_n += share.value;

        // 当当前累计分享值超过预设值时，移除队列前端的分享，以保持队列大小在控制中。
        while *current_n > *self_n {
            let share = self.queue.pop_front().unwrap();
            *current_n -= share.value;
        }

        // 打印性能调试信息。
        debug!("add_share took {} us", start.elapsed().as_micros());
        // 打印当前累计分享值和预设值的调试信息。
        debug!("n: {} / {}", *current_n, self_n);
    }
}

#[derive(Serialize, Deserialize, Clone, Eq, PartialEq, Hash)]
struct Null {}

// 定义会计消息的枚举类型，用于在会计系统中传递不同种类的消息
pub enum AccountingMessage {
    // 新股消息，包含股票代码（String）和发行数量（u64）
    NewShare(String, u64),
    // 设置参数N的消息，用于调整系统某些参数，N为无符号整型
    SetN(u64),
    // 提交新解决方案的消息，包含一个PuzzleSolutions类型的解决方案，用于解决特定的谜题
    NewSolution(PuzzleSolutions<CanaryV0>),
    // 退出消息，表示发送此消息的实体希望退出当前会计系统
    Exit,
}

// 定义支付间隔时间，单位为秒
#[cfg(feature = "db")]
static PAY_INTERVAL: Duration = Duration::from_secs(60);

// Accounting系统的核心实现，负责计算矿工的收益和管理解决方案的记录。
#[allow(clippy::type_complexity)]
pub struct Accounting {
    // PPLNS算法实例，用于计算矿工的收益。
    pplns: Arc<TokioRwLock<PPLNS>>,
    // 数据库实例，用于存储解决方案和其他持久化数据。
    #[cfg(feature = "db")]
    database: Arc<DB>,
    // 用于发送会计消息的通道。
    sender: Sender<AccountingMessage>,
    // 用于缓存当前轮次的矿工和份额信息。
    round_cache: TokioRwLock<Cache<Null, (u32, HashMap<String, u64>)>>,
    // 用于控制退出流程的锁。
    exit_lock: Arc<AtomicBool>,
}

// 初始化Accounting系统。
// 这里将创建必要的PPLNS实例，数据库实例（如果启用），并启动后台任务来处理消息和定期保存PPLNS状态。
impl Accounting {
    pub fn init() -> Arc<Accounting> {
        // 根据feature标志决定是否初始化数据库。
        #[cfg(feature = "db")]
        let database = Arc::new(DB::init());

        // 初始化PPLNS算法实例。
        let pplns = Arc::new(TokioRwLock::new(PPLNS::load()));

        // 创建用于通信的通道。
        let (sender, mut receiver) = channel(1024);

        // 初始化Accounting实例。
        let accounting = Accounting {
            pplns,
            #[cfg(feature = "db")]
            database,
            sender,
            round_cache: TokioRwLock::new(Cache::new(Duration::from_secs(10))),
            exit_lock: Arc::new(AtomicBool::new(false)),
        };

        // 启动一个后台任务来处理接收的消息。
        let pplns = accounting.pplns.clone();
        #[cfg(feature = "db")]
        let database = accounting.database.clone();
        let exit_lock = accounting.exit_lock.clone();
        task::spawn(async move {
            while let Some(request) = receiver.recv().await {
                match request {
                    NewShare(address, value) => {
                        pplns.write().await.add_share(Share::init(value, address.clone()));
                        debug!("Recorded share from {} with value {}", address, value);
                    }
                    SetN(n) => {
                        pplns.write().await.set_n(n);
                        debug!("Set N to {}", n);
                    }
                    #[allow(unused_variables)]
                    NewSolution(commitment) => {
                        let pplns = pplns.read().await.clone();
                        let (_, address_shares) = Accounting::pplns_to_provers_shares(&pplns);

                        #[cfg(feature = "db")]
                        if let Err(e) = database.save_solution(commitment, address_shares).await {
                            error!("Failed to save block reward : {}", e);
                        } else {
                            info!("Recorded solution {}", commitment);
                        }
                    }
                    Exit => {
                        receiver.close();
                        let _ = pplns.read().await.save();
                        exit_lock.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
        });

        // 启动一个定时任务来备份PPLNS状态。
        let pplns = accounting.pplns.clone();
        task::spawn(async move {
            loop {
                sleep(Duration::from_secs(60)).await;
                if let Err(e) = pplns.read().await.save() {
                    error!("Unable to backup pplns: {}", e);
                }
            }
        });

        let res = Arc::new(accounting);

        // 如果启用了数据库功能，启动支付循环。
        #[cfg(feature = "db")]
        task::spawn(Accounting::payout_loop(res.clone()));

        res
    }

    // 返回发送会计消息的通道。
    pub fn sender(&self) -> Sender<AccountingMessage> {
        self.sender.clone()
    }

    // 等待退出信号。
    pub async fn wait_for_exit(&self) {
        while !self.exit_lock.load(std::sync::atomic::Ordering::SeqCst) {
            sleep(Duration::from_millis(100)).await;
        }
    }

    // 将PPLNS内部数据转换为矿工的份额信息。
    fn pplns_to_provers_shares(pplns: &PPLNS) -> (u32, HashMap<String, u64>) {
        let mut address_shares = HashMap::new();

        let time = Instant::now();
        pplns.queue.iter().for_each(|share| {
            if let Some(shares) = address_shares.get_mut(&share.owner) {
                *shares += share.value;
            } else {
                address_shares.insert(share.clone().owner, share.value);
            }
        });
        debug!("PPLNS to Provers shares took {} us", time.elapsed().as_micros());

        (address_shares.len() as u32, address_shares)
    }

    // 获取当前轮次的统计信息。
    pub async fn current_round(&self) -> Value {
        let pplns = self.pplns.clone().read().await.clone();
        let cache = self.round_cache.read().await.get(Null {});
        let (provers, shares) = match cache {
            Some(cache) => cache,
            None => {
                let result = Accounting::pplns_to_provers_shares(&pplns);
                self.round_cache.write().await.set(Null {}, result.clone());
                result
            }
        };
        json!({
            "n": pplns.n,
            "current_n": pplns.current_n,
            "provers": provers,
            "shares": shares,
        })
    }

    /// 根据提交的承诺（commitment）异步检查解决方案的有效性。
    ///
    /// 本函数通过向本地服务器发送HTTP请求，验证给定承诺所对应的解决方案是否有效。
    /// 如果解决方案有效，它将进一步更新数据库中该解决方案的状态。
    ///
    /// # 参数
    /// `commitment` - 待检查的承诺字符串的引用。
    ///
    /// # 返回值
    /// 返回一个`Result`类型，其中包含一个`bool`值，表示承诺的有效性。
    #[cfg(feature = "db")]
    async fn check_solution(&self, commitment: &String) -> Result<bool> {
        // 创建一个新的HTTP客户端实例用于发送请求。
        let client = reqwest::Client::new();

        // 向本地服务器发送GET请求，查询给定承诺的有效性。
        let result = &client
            .get(format!("http://127.0.0.1:8001/commitment?commitment={}", commitment))
            .send()
            .await?
            .json::<Value>()
            .await?;

        // 检查服务器返回的结果是否为`null`，即检查承诺是否有效。
        let is_valid = result.as_null().is_none();

        // 如果承诺有效，则更新数据库中该承诺的状态为有效，并记录相关的高度和奖励信息。
        if is_valid {
            self.database
                .set_solution_valid(
                    commitment,
                    true,
                    Some(result["height"].as_u64().ok_or_else(|| anyhow!("height"))? as u32),
                    Some(result["reward"].as_u64().ok_or_else(|| anyhow!("reward"))?),
                )
                .await?;
        } else {
            // 如果承诺无效，则更新数据库中该承诺的状态为无效。
            self.database.set_solution_valid(commitment, false, None, None).await?;
        }

        // 返回承诺的有效性结果。
        Ok(is_valid)
    }

    /// 在启用数据库功能的特征时，异步执行付款循环。
    /// 此循环定期检查数据库中是否有应该付款的解决方案，并尝试执行付款。
    #[cfg(feature = "db")]
    async fn payout_loop(self: Arc<Accounting>) {
        'forever: loop {
            // 信息级别日志，记录付款循环的启动
            info!("Running payout loop");
            // 尝试获取应该付款的区块列表
            let blocks = self.database.get_should_pay_solutions().await;
            // 如果获取失败，则记录错误并等待一段时间后继续循环
            if blocks.is_err() {
                error!("Unable to get should pay blocks: {}", blocks.unwrap_err());
                sleep(PAY_INTERVAL).await;
                continue;
            }
            // 遍历获取到的区块列表
            for (id, commitment) in blocks.unwrap() {
                // 检查解决方案的有效性
                let valid = self.check_solution(&commitment).await;
                // 如果检查失败，则记录错误并等待一段时间后继续整个循环
                if valid.is_err() {
                    error!("Unable to check solution: {}", valid.unwrap_err());
                    sleep(PAY_INTERVAL).await;
                    continue 'forever;
                }
                // 解析解决方案的有效性结果
                let valid = valid.unwrap();
                // 如果解决方案有效，则尝试执行付款
                if valid {
                    match self.database.pay_solution(id).await {
                        // 如果付款成功，则记录付款信息
                        Ok(_) => {
                            info!("Paid solution {}", commitment);
                        }
                        // 如果付款失败，则记录错误并等待一段时间后继续整个循环
                        Err(e) => {
                            error!("Unable to pay solution {}: {}", id, e);
                            sleep(PAY_INTERVAL).await;
                            continue 'forever;
                        }
                    }
                }
            }

            // 在处理完所有区块后，等待一段时间再开始下一轮循环
            sleep(PAY_INTERVAL).await;
        }
    }
}
