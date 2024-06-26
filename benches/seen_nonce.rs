#[macro_use]
extern crate criterion;

use std::sync::Arc;
use criterion::Criterion;
use flurry::HashSet;
use rand::thread_rng;
use snarkvm::prelude::{CanaryV0, Network};
use snarkvm::utilities::Uniform;

/// 生成一个假的nonce字符串。
///
/// 该函数用于模拟区块链系统中nonce的生成过程，nonce用于确保交易的唯一性。
/// 这里使用了随机数生成器来模拟nonce的生成，返回的nonce是一个字符串格式。
fn fake_nonce() -> String {
    // 从随机数生成器中生成一个假的nonce值
    let nonce: <CanaryV0 as Network>::PoSWNonce = Uniform::rand(&mut thread_rng());
    // 将nonce转换为字符串格式并返回
    nonce.to_string()
}

/// 对已见过的nonce进行基准测试。
///
/// 该函数用于测试在一个大型集合中插入新nonce的性能。
/// 这对于评估区块链系统中处理nonce的能力非常重要，因为nonce的处理直接影响到交易处理的性能。
fn seen_nonce_benchmark(c: &mut Criterion) {
    // 创建一个共享的HashSet，用于存储已见过的nonce
    let nonce_seen = Arc::new(HashSet::with_capacity(10 << 20));
    // 进行基准测试，测量插入新nonce的性能
    c.bench_function("seen_nonce", |b| b.iter(|| nonce_seen.pin().insert(fake_nonce())));
}

// 定义基准测试组
criterion_group!(nonce, seen_nonce_benchmark);
// 设置主基准测试
criterion_main!(nonce);
