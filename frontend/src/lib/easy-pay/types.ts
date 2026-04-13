export interface EasyPayCreateParams {
  pid: string;
  cid?: string;
  type: 'alipay' | 'wxpay';
  out_trade_no: string;
  notify_url: string;
  name: string;
  money: string;
  clientip: string;
  return_url: string;
  sign?: string;
  sign_type?: string;
}

export interface EasyPayCreateResponse {
  code: number;
  msg?: string;
  trade_no: string;
  O_id?: string;
  payurl?: string;
  payurl2?: string;
  qrcode?: string;
  img?: string;
}

export interface EasyPayNotifyParams {
  pid: string;
  name: string;
  money: string;
  out_trade_no: string;
  trade_no: string;
  param?: string;
  trade_status: string;
  type: string;
  sign: string;
  sign_type: string;
}

export interface EasyPayQueryResponse {
  code: number;
  msg?: string;
  trade_no: string;
  out_trade_no: string;
  type: string;
  pid: string;
  addtime: string;
  endtime: string;
  name: string;
  money: string;
  status: number;
  param?: string;
  buyer?: string;
}

export interface EasyPayRefundResponse {
  code: number;
  msg: string;
}
