import React, { useState, useRef, useCallback, useEffect } from 'react';

export default function ZoomableImage({ src, className, alt = '' }) {
  const [scale, setScale] = useState(1);
  const [translate, setTranslate] = useState({ x: 0, y: 0 });
  const [dragging, setDragging] = useState(false);
  const dragStart = useRef({ x: 0, y: 0 });
  const translateStart = useRef({ x: 0, y: 0 });
  const wrapRef = useRef(null);

  // Reset zoom when image changes
  useEffect(() => {
    setScale(1);
    setTranslate({ x: 0, y: 0 });
  }, [src]);

  const clampTranslate = useCallback((tx, ty, s) => {
    if (s <= 1) return { x: 0, y: 0 };
    const wrap = wrapRef.current;
    if (!wrap) return { x: tx, y: ty };
    const img = wrap.querySelector('img');
    if (!img) return { x: tx, y: ty };
    // How much the image overflows the container when scaled
    const maxX = Math.max(0, (img.offsetWidth * s - wrap.offsetWidth) / 2);
    const maxY = Math.max(0, (img.offsetHeight * s - wrap.offsetHeight) / 2);
    return {
      x: Math.max(-maxX, Math.min(maxX, tx)),
      y: Math.max(-maxY, Math.min(maxY, ty)),
    };
  }, []);

  const handleWheel = useCallback((e) => {
    e.preventDefault();
    e.stopPropagation();
    setScale(prev => {
      const delta = e.deltaY > 0 ? -0.15 : 0.15;
      const next = Math.min(5, Math.max(1, prev + delta));
      if (next <= 1) setTranslate({ x: 0, y: 0 });
      else setTranslate(t => clampTranslate(t.x, t.y, next));
      return next;
    });
  }, [clampTranslate]);

  const handleMouseDown = useCallback((e) => {
    if (scale <= 1) return;
    e.preventDefault();
    setDragging(true);
    dragStart.current = { x: e.clientX, y: e.clientY };
    translateStart.current = { ...translate };
  }, [scale, translate]);

  const handleMouseMove = useCallback((e) => {
    if (!dragging) return;
    const dx = e.clientX - dragStart.current.x;
    const dy = e.clientY - dragStart.current.y;
    setTranslate(clampTranslate(
      translateStart.current.x + dx,
      translateStart.current.y + dy,
      scale
    ));
  }, [dragging, scale, clampTranslate]);

  const handleMouseUp = useCallback(() => {
    setDragging(false);
  }, []);

  // Release drag if mouse leaves window
  useEffect(() => {
    if (!dragging) return;
    const up = () => setDragging(false);
    window.addEventListener('mouseup', up);
    return () => window.removeEventListener('mouseup', up);
  }, [dragging]);

  const cursor = scale > 1 ? (dragging ? 'grabbing' : 'grab') : 'default';

  return (
    <div
      ref={wrapRef}
      className="zoomable-img-wrap"
      onWheel={handleWheel}
      onMouseDown={handleMouseDown}
      onMouseMove={handleMouseMove}
      onMouseUp={handleMouseUp}
      style={{ cursor }}
    >
      <img
        className={className}
        src={src}
        alt={alt}
        draggable={false}
        style={{
          transform: `scale(${scale}) translate(${translate.x / scale}px, ${translate.y / scale}px)`,
          transformOrigin: 'center center',
        }}
      />
      {scale > 1 && (
        <span className="zoomable-badge">{Math.round(scale * 100)}%</span>
      )}
    </div>
  );
}
