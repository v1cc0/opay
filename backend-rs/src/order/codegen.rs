use uuid::Uuid;

pub fn generate_recharge_code(order_id: &str) -> String {
    let prefix = "s2p_";
    let random = Uuid::new_v4().simple().to_string();
    let random = &random[..8];
    let max_id_length = 32usize.saturating_sub(prefix.len() + random.len());
    let truncated = order_id.replace('-', "");
    let truncated = &truncated[..truncated.len().min(max_id_length)];
    format!("{}{}{}", prefix, truncated, random)
}
