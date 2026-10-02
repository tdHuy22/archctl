import { createOrder } from '../order/create-order'
import { profile } from '../user/profile'

// Vi phạm: `deny * -> Stripe except Payment`. Web phải đi qua Payment.
import Stripe from 'stripe'

// Chưa phân giải được: alias tsconfig paths.
import { formatMoney } from '@/lib/format'

// Vi phạm: `contain Timeout` của boundary Checkout. Lỗi hạ tầng không được lọt
// ra khỏi vùng — tầng Web lẽ ra chỉ thấy lỗi nghiệp vụ.
const Timeout = 'timeout'

export async function checkout(items: unknown[]) {
  const who = await profile()
  const order = await createOrder(items, who)
  return { total: formatMoney(order.total), fallback: Timeout }
}

export const stripe = new Stripe(process.env.STRIPE_KEY ?? '')
