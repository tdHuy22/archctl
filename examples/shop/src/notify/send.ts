export async function send(event: string) {
  return { event, at: Date.now() }
}
