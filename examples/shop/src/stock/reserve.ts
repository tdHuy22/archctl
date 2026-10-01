import { send } from '../notify/send'

// Vi phạm: Stock đã vào diện kiểm nhờ `allow Stock -> Notify`, và không có
// allow nào cho Stock -> Payment.
import { charge } from '../payment/charge'

export async function reserve(items: unknown[]) {
  await send('stock.reserved')
  return charge(items)
}
