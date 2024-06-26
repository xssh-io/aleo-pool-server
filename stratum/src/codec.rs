use std::io;

use bytes::BytesMut;
use downcast_rs::{impl_downcast, DowncastSync};
use erased_serde::Serialize as ErasedSerialize;
use json_rpc_types::{Id, Request, Response, Version};
use serde::{ser::SerializeSeq, Deserialize, Serialize};
use serde_json::Value;
use tokio_util::codec::{AnyDelimiterCodec, Decoder, Encoder};

use crate::message::StratumMessage;

/// `StratumCodec`是一个封装了任意分隔符编解码器的结构体，用于处理特定的矿工任务通信协议。
pub struct StratumCodec {
    /// `codec`字段是一个任意分隔符编解码器，它实现了对矿工任务请求和响应的编解码。
    codec: AnyDelimiterCodec,
}

/// 为`StratumCodec`结构体实现`Default`特征，提供一个默认构造方法。
impl Default for StratumCodec {

    /// 创建一个具有默认设置的`StratumCodec`新实例。
    ///
    /// # 返回值
    /// 返回一个`StratumCodec`的默认实例。
    fn default() -> Self {
        Self {
            // 选择AnyDelimiterCodec作为编解码器，使用换行符作为起始和结束的分隔符，
            // 并将消息的最大长度设置为4096字节。这一配置依据了Stratum挖矿协议中消息的常见大小，
            // 其中Notify消息约为400字节，submit消息约为1750字节，4096字节足以应对所有消息的需求。
            // TODO: 再次验证该设置
            codec: AnyDelimiterCodec::new_with_max_length(vec![b'\n'], vec![b'\n'], 4096),
        }
    }
}

#[derive(Serialize, Deserialize)]
struct NotifyParams(String, String, Option<String>, bool);

#[derive(Serialize, Deserialize)]
struct SubscribeParams(String, String, Option<String>);

pub trait BoxedType: ErasedSerialize + Send + DowncastSync {}
erased_serde::serialize_trait_object!(BoxedType);
impl_downcast!(sync BoxedType);

impl BoxedType for String {}
impl BoxedType for Option<u64> {}
impl BoxedType for Option<String> {}

/**
 * ResponseParams枚举定义了不同类型的响应参数。
 *
 * 此枚举支持三种不同的形式：
 * - Bool: 表示布尔类型的响应参数。
 * - Array: 表示由实现了BoxedType特质的元素组成的向量，这种形式允许灵活的类型数组作为响应参数。
 * - Null: 表示空值的响应参数。
 */
pub enum ResponseParams {
    Bool(bool),
    Array(Vec<Box<dyn BoxedType>>),
    Null,
}

/// 实现 `Serialize` trait 用于 `ResponseParams` 类型，使其可以被序列化。
impl Serialize for ResponseParams {

    /// 序列化 `ResponseParams` 对象。
    ///
    /// 根据 `ResponseParams` 的具体类型，选择合适的序列化方式：
    /// - 如果是 `Bool` 类型，则序列化为布尔值。
    /// - 如果是 `Array` 类型，则序列化为数组。
    /// - 如果是 `Null` 类型，则序列化为 `None`。
    ///
    /// # 参数
    /// - `serializer`: 一个序列化器，用于实际的序列化操作。
    ///
    /// # 返回值
    /// 返回序列化结果，如果序列化成功，则为 `Ok`；如果失败，则为 `Error`。
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            ResponseParams::Bool(b) => serializer.serialize_bool(*b),
            ResponseParams::Array(v) => {
                // 初始化序列化器，指定数组长度
                let mut seq = serializer.serialize_seq(Some(v.len()))?;
                // 遍历数组，序列化每个元素
                for item in v {
                    seq.serialize_element(item)?;
                }
                // 结束序列化数组
                seq.end()
            }
            ResponseParams::Null => serializer.serialize_none(),
        }
    }
}

/// 实现 `Deserialize` trait 用于 `ResponseParams` 类型，使其可以被反序列化。
impl<'de> Deserialize<'de> for ResponseParams {

    /// 反序列化 `ResponseParams`。
    ///
    /// # 参数
    /// `deserializer` - 一个序列化器，用于读取并转换数据。
    ///
    /// # 返回值
    /// 返回 `Result<Self, D::Error>`，其中 `Self` 是反序列化后的 `ResponseParams` 实例，
    /// `D::Error` 是序列化器可能产生的错误。
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        // 尝试将数据反序列化为一个通用的 `Value` 类型。
        let value = Value::deserialize(deserializer)?;

        // 根据 `Value` 的具体类型，将其转换为 `ResponseParams` 的相应形态。
        match value {
            Value::Bool(b) => Ok(ResponseParams::Bool(b)),
            Value::Array(a) => {
                // 初始化一个动态大小的向量，用于存储反序列化后的数据。
                let mut vec: Vec<Box<dyn BoxedType>> = Vec::new();

                // 遍历数组中的每个元素，并根据其类型转换为相应的 `BoxedType` 形态。
                a.iter().for_each(|v| match v {
                    Value::Null => vec.push(Box::new(None::<String>)),
                    Value::String(s) => vec.push(Box::new(s.clone())),
                    Value::Number(n) => vec.push(Box::new(n.as_u64())),
                    _ => {}
                });

                // 将转换后的数据封装为 `ResponseParams::Array` 形态返回。
                Ok(ResponseParams::Array(vec))
            }
            // 如果遇到无法处理的类型，则返回一个自定义错误。
            Value::Null => Ok(ResponseParams::Null),
            _ => Err(serde::de::Error::custom("invalid response params")),
        }
    }
}

/// 实现了对StratumMessage类型进行编码的Encoder trait。
impl Encoder<StratumMessage> for StratumCodec {

    /// 错误类型定义为io::Error。
    type Error = io::Error;

    /// 对StratumMessage进行编码，并写入到BytesMut缓冲区中。
    ///
    /// # 参数
    /// - `self`: 当前的StratumCodec实例，它是一个可变引用。
    /// - `item`: 需要编码的StratumMessage对象。
    /// - `dst`: 目标缓冲区，编码后的数据将写入到这里。
    ///
    /// # 返回值
    /// 返回一个Result，表示编码操作是否成功。成功时，返回(); 错误时，返回对应的io::Error。
    fn encode(&mut self, item: StratumMessage, dst: &mut BytesMut) -> Result<(), Self::Error> {
        // 根据不同的StratumMessage类型，构造相应的JSON-RPC请求，并将其编码为字节串。
        let bytes = match item {
            StratumMessage::Subscribe(id, user_agent, protocol_version, session_id) => {
                let request = Request {
                    jsonrpc: Version::V2,
                    method: "mining.subscribe",
                    params: Some(SubscribeParams(user_agent, protocol_version, session_id)),
                    id: Some(id),
                };
                serde_json::to_vec(&request).unwrap_or_default()
            }
            StratumMessage::Authorize(id, worker_name, worker_password) => {
                let request = Request {
                    jsonrpc: Version::V2,
                    method: "mining.authorize",
                    params: Some(vec![worker_name, worker_password]),
                    id: Some(id),
                };
                serde_json::to_vec(&request).unwrap_or_default()
            }
            StratumMessage::SetTarget(difficulty_target) => {
                let request = Request {
                    jsonrpc: Version::V2,
                    method: "mining.set_target",
                    params: Some(vec![difficulty_target]),
                    id: None,
                };
                serde_json::to_vec(&request).unwrap_or_default()
            }
            StratumMessage::Notify(job_id, epoch_challenge, address, clean_jobs) => {
                let request = Request {
                    jsonrpc: Version::V2,
                    method: "mining.notify",
                    params: Some(NotifyParams(job_id, epoch_challenge, address, clean_jobs)),
                    id: None,
                };
                serde_json::to_vec(&request).unwrap_or_default()
            }
            StratumMessage::Submit(id, worker_name, job_id, nonce, commitment, proof) => {
                let request = Request {
                    jsonrpc: Version::V2,
                    method: "mining.submit",
                    params: Some(vec![worker_name, job_id, nonce, commitment, proof]),
                    id: Some(id),
                };
                serde_json::to_vec(&request).unwrap_or_default()
            }
            StratumMessage::Response(id, result, error) => match error {
                Some(error) => {
                    let response = Response::<(), ()>::error(Version::V2, error, Some(id));
                    serde_json::to_vec(&response).unwrap_or_default()
                }
                None => {
                    let response = Response::<Option<ResponseParams>, ()>::result(Version::V2, result, Some(id));
                    serde_json::to_vec(&response).unwrap_or_default()
                }
            },
        };
        // 将bytes转换为UTF-8字符串，如果转换失败，则返回错误。
        let string =
            std::str::from_utf8(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        // 使用codec的encode方法将字符串编码，并写入到dst缓冲区中。
        // 如果编码失败，则返回错误。
        self.codec
            .encode(string, dst)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        // 编码成功，返回()。
        Ok(())
    }
}

/**
 * 从 `Value` 中提取字符串值。
 *
 * 此函数尝试从给定的 `Value` 中提取一个字符串值。如果 `Value` 是字符串类型，则函数成功并返回该字符串的副本；
 * 否则，函数返回一个错误，表明参数不是字符串类型。
 *
 * @param value 需要被解析的 `Value` 对象。
 * @return 如果成功提取到字符串，返回 `Ok` 包含字符串值；如果 `Value` 不是字符串类型，则返回 `Err`，其中包含一个描述错误的信息。
 */
fn unwrap_str_value(value: &Value) -> Result<String, io::Error> {
    match value {
        Value::String(s) => Ok(s.clone()),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Param is not str")),
    }
}

/**
 * 从 `Value` 中提取布尔值。
 *
 * 此函数尝试从给定的 `Value` 中提取一个布尔值。如果 `Value` 是一个布尔类型，则成功返回该布尔值；
 * 否则，返回一个错误，表明参数不是布尔类型。
 *
 * @param value 需要被解析的 `Value` 对象。
 * @return 如果成功提取到布尔值，返回 `Ok(true)` 或 `Ok(false)`；如果 `Value` 不是布尔类型，则返回一个包含错误信息的 `Err`。
 */
fn unwrap_bool_value(value: &Value) -> Result<bool, io::Error> {
    match value {
        Value::Bool(b) => Ok(*b),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Param is not bool")),
    }
}

/**
 * 从 `Value` 中提取 `u64` 类型的值。
 *
 * 此函数尝试将给定的 `Value` 解包为 `u64` 类型。如果 `Value` 是一个数字类型，则尝试将其转换为 `u64`。
 * 如果转换成功，函数返回 `Ok` 包含转换后的值；如果转换失败或 `Value` 不是数字类型，则返回一个表示
 * 数据无效的错误。
 *
 * @param value 需要解包的 `Value` 对象。
 * @return 如果成功解包并转换为 `u64`，则返回 `Ok` 包含该值；否则返回一个 `Err`，其中包含描述错误的信息。
 */
fn unwrap_u64_value(value: &Value) -> Result<u64, io::Error> {
    match value {
        Value::Number(n) => Ok(n
            .as_u64()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "Param is not u64"))?),
        _ => Err(io::Error::new(io::ErrorKind::InvalidData, "Param is not u64")),
    }
}

impl Decoder for StratumCodec {

    // 定义解码后的Item类型为StratumMessage
    type Item = StratumMessage;

    // 定义解码错误的类型为io::Error
    type Error = io::Error;

    /**
     * 解码矿工与矿池之间的通信消息。
     *
     * 该方法尝试从BytesMut中解码一条Stratum消息。它首先使用底层编解码器解码字符串，
     * 然后尝试将该字符串解析为JSON对象。根据JSON对象的内容，它将解析为不同的StratumMessage类型。
     *
     * @param src 待解码的数据，作为一个可变的BytesMut对象。
     * @returns 解码后的StratumMessage选项，或者在解码过程中遇到的错误。
     */
    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        // 使用底层编解码器尝试解码源数据
        let string = self
            .codec
            .decode(src)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        // 如果解码结果为空，则直接返回None
        if string.is_none() {
            return Ok(None);
        }

        // 解码结果转换为字节切片，并尝试解析为JSON对象
        let bytes = string.unwrap();
        let json = serde_json::from_slice::<serde_json::Value>(&bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

        // 检查解析后的JSON是否为对象类型
        if !json.is_object() {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "Not an object"));
        }

        // 获取JSON对象
        let object = json.as_object().unwrap();

        // 根据JSON对象是否包含"method"键来区分请求和响应
        let result = if object.contains_key("method") {
            // 解析为请求类型
            let request = serde_json::from_value::<Request<Vec<Value>>>(json)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;

            // 提取请求ID、方法名和参数
            let id = request.id;
            let method = request.method.as_str();
            let params = match request.params {
                Some(params) => params,
                None => return Err(io::Error::new(io::ErrorKind::InvalidData, "No params")),
            };

            // 根据方法名解析为具体的StratumMessage类型
            match method {
                "mining.subscribe" => {
                    // 订阅消息处理
                    if params.len() != 3 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params"));
                    }
                    let user_agent = unwrap_str_value(&params[0])?;
                    let protocol_version = unwrap_str_value(&params[1])?;
                    let session_id = match &params[2] {
                        Value::String(s) => Some(s),
                        Value::Null => None,
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params")),
                    };
                    StratumMessage::Subscribe(
                        id.unwrap_or(Id::Num(0)),
                        user_agent,
                        protocol_version,
                        session_id.cloned(),
                    )
                }
                "mining.authorize" => {
                    // 认证消息处理
                    if params.len() != 2 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params"));
                    }
                    let worker_name = unwrap_str_value(&params[0])?;
                    let worker_password = unwrap_str_value(&params[1])?;
                    StratumMessage::Authorize(id.unwrap_or(Id::Num(0)), worker_name, worker_password)
                }
                "mining.set_target" => {
                    // 设置目标消息处理
                    if params.len() != 1 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params"));
                    }
                    let difficulty_target = unwrap_u64_value(&params[0])?;
                    StratumMessage::SetTarget(difficulty_target)
                }
                "mining.notify" => {
                    // 通知消息处理
                    if params.len() != 4 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params"));
                    }
                    let job_id = unwrap_str_value(&params[0])?;
                    let epoch_challenge = unwrap_str_value(&params[1])?;
                    let address = match &params[2] {
                        Value::String(s) => Some(s),
                        Value::Null => None,
                        _ => return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params")),
                    };
                    let clean_jobs = unwrap_bool_value(&params[3])?;
                    StratumMessage::Notify(job_id, epoch_challenge, address.cloned(), clean_jobs)
                }
                "mining.submit" => {
                    // 提交消息处理
                    if params.len() != 5 {
                        return Err(io::Error::new(io::ErrorKind::InvalidData, "Invalid params"));
                    }
                    let worker_name = unwrap_str_value(&params[0])?;
                    let job_id = unwrap_str_value(&params[1])?;
                    let nonce = unwrap_str_value(&params[2])?;
                    let commitment = unwrap_str_value(&params[3])?;
                    let proof = unwrap_str_value(&params[4])?;
                    StratumMessage::Submit(id.unwrap_or(Id::Num(0)), worker_name, job_id, nonce, commitment, proof)
                }
                _ => {
                    return Err(io::Error::new(io::ErrorKind::InvalidData, "Unknown method"));
                }
            }
        } else {
            let response = serde_json::from_value::<Response<ResponseParams, ()>>(json)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            let id = response.id;
            match response.payload {
                Ok(payload) => StratumMessage::Response(id.unwrap_or(Id::Num(0)), Some(payload), None),
                Err(error) => StratumMessage::Response(id.unwrap_or(Id::Num(0)), None, Some(error)),
            }
        };
        Ok(Some(result))
    }
}
