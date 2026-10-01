import { reserve } from '../stock/reserve'
import { charge } from '../payment/charge'
import { send } from '../notify/send'

// Chưa phân giải được: không có file nào ở đường dẫn này.
import { Cart } from '../shared/cart-types'

export async function createOrder(items: unknown[], who: unknown) {
  await reserve(items)
  const paid = await charge(items)
  await send('order.created')
  return { total: paid.amount } as unknown as Cart
}
