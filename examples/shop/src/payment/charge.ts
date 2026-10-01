import Stripe from 'stripe'

// Hai dòng dưới đây KHÔNG phải import. Nếu bộ quét tính chúng là import thì
// sẽ sinh ra vi phạm Payment -> User, nên đây là phép thử cho lexer.
// import { profile } from '../user/profile'
const docs = "import { profile } from '../user/profile'"
const more = `import { profile } from '../user/profile'`
const ratio = docs.length / more.length / 2

export async function charge(items: unknown[]) {
  const stripe = new Stripe(process.env.STRIPE_KEY ?? '')
  return { amount: items.length * ratio, provider: stripe }
}
