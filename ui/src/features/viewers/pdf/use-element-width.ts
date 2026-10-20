/**
 * The width an element is given, in whole pixels, for a PDF page that fits it (UI M4a, ruling A2).
 * Zero until the first layout measurement, and wherever there is no layout at all.
 */
import { type RefObject, useEffect, useState } from 'react';

export function useElementWidth(ref: RefObject<HTMLElement | null>): number {
  const [width, setWidth] = useState(0);
  useEffect(() => {
    const element = ref.current;
    if (element === null) return;
    const measure = () => setWidth(Math.floor(element.getBoundingClientRect().width));
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [ref]);
  return width;
}
