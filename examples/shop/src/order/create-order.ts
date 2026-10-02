import { reserve } from '../stock/reserve'
import { charge } from '../payment/charge'
import { send } from '../notify/send'

// Chưa phân giải được: không có file nào ở đường dẫn này.
import { Cart } from '../shared/cart-types'

// invariant state [Order]: CREATED -> RESERVED -> PAID -> SHIPPED
export const CREATED = 'created'
export const RESERVED = 'reserved'
export const PAID = 'paid'
export const SHIPPED = 'shipped'

export function validateCart(items: unknown[]) {
  return items.length > 0
}

export function confirmOrder(id: string) {
  return { id, status: SHIPPED }
}

export async function createOrder(items: unknown[], who: unknown) {
  validateCart(items)
  await reserve(items)
  const paid = await charge(items)
  await send('order.created')
  confirmOrder('o_1')
  return { total: paid.amount } as unknown as Cart
}
