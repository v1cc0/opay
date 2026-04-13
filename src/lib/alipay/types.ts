/** 支付宝电脑网站支付 bizContent */
export interface AlipayTradePagePayBizContent {
  out_trade_no: string;
  product_code: 'FAST_INSTANT_TRADE_PAY';
  total_amount: string;
  subject: string;
  body?: string;
}

/** 支付宝统一响应结构 */
export interface AlipayResponse {
  code: string;
  msg: string;
  sub_code?: string;
  sub_msg?: string;
}

/** alipay.trade.query 响应 */
export interface AlipayTradeQueryResponse extends AlipayResponse {
  trade_no?: string;
  out_trade_no?: string;
  trade_status?: string; // WAIT_BUYER_PAY, TRADE_CLOSED, TRADE_SUCCESS, TRADE_FINISHED
  total_amount?: string;
  send_pay_date?: string;
}

/** alipay.trade.refund 响应 */
export interface AlipayTradeRefundResponse extends AlipayResponse {
  trade_no?: string;
  out_trade_no?: string;
  refund_fee?: string;
  fund_change?: string; // Y/N
}

/** alipay.trade.close 响应 */
export interface AlipayTradeCloseResponse extends AlipayResponse {
  trade_no?: string;
  out_trade_no?: string;
}

/** 异步通知参数 */
export interface AlipayNotifyParams {
  notify_time: string;
  notify_type: string;
  notify_id: string;
  app_id: string;
  charset: string;
  version: string;
  sign_type: string;
  sign: string;
  trade_no: string;
  out_trade_no: string;
  trade_status: string;
  total_amount: string;
  receipt_amount?: string;
  buyer_pay_amount?: string;
  gmt_payment?: string;
  [key: string]: string | undefined;
}
