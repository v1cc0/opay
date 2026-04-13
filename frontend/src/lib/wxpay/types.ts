export interface WxpayPcOrderParams {
  out_trade_no: string;
  description: string;
  notify_url: string;
  amount: number; // in yuan, will be converted to fen
}

export interface WxpayH5OrderParams {
  out_trade_no: string;
  description: string;
  notify_url: string;
  amount: number; // in yuan
  payer_client_ip: string;
}

export interface WxpayRefundParams {
  out_trade_no: string;
  out_refund_no: string;
  amount: number; // refund amount in yuan
  total: number; // original total in yuan
  reason?: string;
}

export interface WxpayNotifyPayload {
  id: string;
  create_time: string;
  event_type: string;
  resource: {
    algorithm: string;
    ciphertext: string;
    nonce: string;
    associated_data: string;
  };
}

export interface WxpayNotifyResource {
  appid: string;
  mchid: string;
  out_trade_no: string;
  transaction_id: string;
  trade_type: string;
  trade_state: string;
  trade_state_desc: string;
  bank_type: string;
  success_time: string;
  payer: { openid?: string };
  amount: {
    total: number; // in fen
    payer_total: number;
    currency: string;
  };
}
