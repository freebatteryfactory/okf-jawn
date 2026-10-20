/** Read the containing iframe and ancestor animations in that frame's own document. */
function frameBox(node) {
  const active = (target) => target.getAnimations().filter((animation) => animation.playState === 'running' || animation.pending).length;
  let animations = active(node);
  for (let parent = node.parentElement; parent; parent = parent.parentElement) animations += active(parent);
  return { height: node.clientHeight, rect_height: node.getBoundingClientRect().height, animations };
}

/** Browser defaults are injectable only so the serialized reader can use small DOM fixtures. */
function appBox({ doc = document, height = innerHeight } = {}) {
  return {
    height,
    content: Math.max(doc.documentElement.scrollHeight, doc.body.scrollHeight),
    animations: doc.getAnimations().filter((animation) => animation.playState === 'running' || animation.pending).length,
  };
}

/** Measure every containing iframe; dispose every acquired handle even when a read fails. */
export async function measureViewport(frame) {
  const elements = [];
  try {
    for (let child = frame; child.parentFrame(); child = child.parentFrame()) elements.push(await child.frameElement());
    const boxes = await Promise.all(elements.map((element) => element.evaluate(frameBox)));
    const app = await frame.evaluate(appBox);
    return {
      heights: boxes.map((box) => [box.height, box.rect_height]),
      viewport_height: Math.min(app.height, ...boxes.map((box) => box.height)),
      content_height: app.content,
      active_animations: app.animations + boxes.reduce((sum, box) => sum + box.animations, 0),
    };
  } finally {
    await Promise.all(elements.map((element) => element.dispose()));
  }
}
