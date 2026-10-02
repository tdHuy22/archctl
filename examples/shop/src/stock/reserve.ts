import { send } from '../notify/send'

// Vi phạm: Stock đã vào diện kiểm nhờ `allow Stock -> Notify`, và không có
// allow nào cho Stock -> Payment.
import { charge } from '../payment/charge'

export async function reserveStock(items: unknown[]) {
  await send('stock.reserved')
  return items.length
}

export async function releaseStock(items: unknown[]) {
  await send('stock.released')
  return items.length
}

export const reserve = reserveStock

export async function withCharge(items: unknown[]) {
  return charge(items)
}
