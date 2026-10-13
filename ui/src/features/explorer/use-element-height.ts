/** The rendered height of an element, for components (the virtualized tree) that need a number. */
import { type RefObject, useEffect, useState } from 'react';

/** Used until the first layout measurement arrives, and wherever there is no layout at all. */
const BEFORE_LAYOUT = 480;

export function useElementHeight(ref: RefObject<HTMLElement | null>): number {
  const [height, setHeight] = useState(BEFORE_LAYOUT);
  useEffect(() => {
    const element = ref.current;
    if (element === null) return;
    const measure = () => {
      const measured = element.getBoundingClientRect().height;
      if (measured > 0) setHeight(Math.floor(measured));
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return height;
}
