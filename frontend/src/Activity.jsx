import React, {useEffect, useRef} from 'react';

export function BorderBeam() {
  return <span className="border-beam" aria-hidden="true"><span /></span>;
}

export function AnimatedCount({value}) {
  const element = useRef(null);
  const previous = useRef(value);
  useEffect(() => {
    if (previous.current === value) return;
    previous.current = value;
    const preference = matchMedia('(prefers-reduced-motion: reduce)');
    if (preference.matches) return;
    const animation = element.current.animate([{opacity: .35, transform: 'translateY(3px)'}, {opacity: 1, transform: 'translateY(0)'}], {duration: 240, easing: 'ease-out'});
    const stop = () => {if (preference.matches) animation.cancel();};
    preference.addEventListener('change', stop);
    return () => {animation.cancel(); preference.removeEventListener('change', stop);};
  }, [value]);
  return <span className="animated-count"><span ref={element}>{value}</span></span>;
}
