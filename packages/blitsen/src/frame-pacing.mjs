/** Advances a 60 Hz deadline and returns how long remains before it. */
export function frameDelay(pacing, now) {
  const frameInterval = 1000 / 60;
  pacing.nextFrame += frameInterval;
  if (pacing.nextFrame < now - frameInterval) pacing.nextFrame = now;
  return Math.max(0, pacing.nextFrame - now);
}
