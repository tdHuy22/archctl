export async function send(event: string) {
  return { event, at: Date.now() }
}

export async function sendEmail(to: string) {
  return send(`email:${to}`)
}
