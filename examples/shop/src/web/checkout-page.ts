import { createOrder } from '../order/create-order'
import { profile } from '../user/profile'

// Vi phạm: `deny * -> Stripe except Payment`. Web phải đi qua Payment.
import Stripe from 'stripe'

// Chưa phân giải được: alias tsconfig paths.
import { formatMoney } from '@/lib/format'

export async function checkout(items: unknown[]) {
  const who = await profile()
  const order = await createOrder(items, who)
  return formatMoney(order.total)
}

export const stripe = new Stripe(process.env.STRIPE_KEY ?? '')
