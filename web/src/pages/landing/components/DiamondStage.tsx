import { Canvas, useFrame } from "@react-three/fiber";
import { useEffect, useMemo, useRef, useState } from "react";
import * as THREE from "three";

const VERTEX_SHADER = `
  varying vec3 vNormal;
  varying vec3 vWorldPos;
  varying vec3 vViewDir;
  void main() {
    vec4 wp = modelMatrix * vec4(position, 1.0);
    vWorldPos = wp.xyz;
    vNormal   = normalize(mat3(modelMatrix) * normal);
    vViewDir  = normalize(cameraPosition - wp.xyz);
    gl_Position = projectionMatrix * viewMatrix * wp;
  }
`;

const FRAGMENT_SHADER = `
  uniform float uTime;
  uniform vec3  uBase;
  uniform vec3  uShimmer;
  uniform vec3  uHi;
  varying vec3 vNormal;
  varying vec3 vWorldPos;
  varying vec3 vViewDir;

  void main() {
    vec3 N = normalize(vNormal);
    vec3 V = normalize(vViewDir);

    float ndv = abs(dot(N, V));
    float fresnel = pow(1.0 - ndv, 2.6);

    vec3 L1 = normalize(vec3(sin(uTime * 0.35) * 1.2, 0.85, cos(uTime * 0.35) * 1.2));
    vec3 L2 = normalize(vec3(cos(uTime * 0.55 + 1.7) * 1.0, -0.25, sin(uTime * 0.55 + 1.7) * 1.0));
    vec3 L3 = normalize(vec3(0.3, sin(uTime * 0.8) * 0.4 + 0.5, 0.8));

    vec3 H1 = normalize(L1 + V);
    vec3 H2 = normalize(L2 + V);
    vec3 H3 = normalize(L3 + V);

    float s1 = pow(max(dot(N, H1), 0.0), 80.0);
    float s2 = pow(max(dot(N, H2), 0.0), 160.0);
    float s3 = pow(max(dot(N, H3), 0.0), 260.0);

    float d1 = max(dot(N, L1), 0.0);

    vec3 color = uBase;
    color += uShimmer * fresnel * 0.55;
    color += uShimmer * d1 * 0.04;
    color += uHi * s1 * 0.70;
    color += uHi * s2 * 0.95;
    color += uHi * s3 * 1.30;

    gl_FragColor = vec4(color, 1.0);
  }
`;

const DIAMOND_ASCII = `        _______
      .'_/_|_\\_'.
      \\\`\\  |  /\`/
       \`\\\\ | //'
         \`\\|/\`
           \``;

function Diamond({ mouseRef }: { mouseRef: React.MutableRefObject<{ x: number; y: number }> }) {
  const meshRef = useRef<THREE.Mesh>(null);
  const materialRef = useRef<THREE.ShaderMaterial>(null);
  const startRef = useRef(performance.now());

  const geometry = useMemo(() => {
    const g = new THREE.OctahedronGeometry(0.78, 0);
    g.scale(1, 1.45, 1);
    g.computeVertexNormals();
    return g;
  }, []);

  const uniforms = useMemo(
    () => ({
      uTime: { value: 0 },
      uBase: { value: new THREE.Color(0x000000) },
      uShimmer: { value: new THREE.Color(0x8cc0ff) },
      uHi: { value: new THREE.Color(0xbcdcff) },
    }),
    [],
  );

  useFrame(() => {
    const t = (performance.now() - startRef.current) / 1000;
    if (materialRef.current) {
      materialRef.current.uniforms.uTime.value = t;
    }
    if (meshRef.current) {
      meshRef.current.rotation.y = t * 0.42 + mouseRef.current.x * 0.5;
      meshRef.current.rotation.x = Math.sin(t * 0.28) * 0.18 + mouseRef.current.y * 0.3;
      // Bias the diamond slightly downward in the canvas so it sits closer to
      // the section bottom — visually tightens the gap to the hero text below.
      meshRef.current.position.y = -0.35 + Math.sin(t * 0.5) * 0.04;
    }
  });

  return (
    <mesh ref={meshRef} geometry={geometry}>
      <shaderMaterial
        ref={materialRef}
        uniforms={uniforms}
        vertexShader={VERTEX_SHADER}
        fragmentShader={FRAGMENT_SHADER}
      />
    </mesh>
  );
}

const RAMP = " .·,:;-~+=*x×X%#%@▒▓█";
const CELL_W = 8;
const CELL_H = 13;

function hashNoise(x: number, y: number, t: number) {
  const n = Math.sin(x * 12.9898 + y * 78.233 + t * 7.117) * 43758.5453;
  return n - Math.floor(n);
}

export function DiamondStage() {
  const sectionRef = useRef<HTMLElement>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const wrapRef = useRef<HTMLDivElement>(null);
  const asciiRef = useRef<HTMLCanvasElement>(null);
  const webglRef = useRef<HTMLCanvasElement | null>(null);
  const sampleRef = useRef<HTMLCanvasElement | null>(null);
  const mouseRef = useRef({ x: 0, y: 0 });
  const progressRef = useRef(0);

  const [reducedMotion, setReducedMotion] = useState(() => {
    if (typeof window === "undefined") return false;
    return window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  });

  useEffect(() => {
    const mq = window.matchMedia("(prefers-reduced-motion: reduce)");
    const onChange = () => setReducedMotion(mq.matches);
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  // Scroll handler: progress 0 → 1 across the diamond section's sticky pin range,
  // plus the top-down mask wipe on the WebGL canvas. The pin range is
  // sectionH - stageH (how far the section can scroll while the stage stays
  // pinned at top:0), not viewport height — the stage is smaller than vh now.
  useEffect(() => {
    const section = sectionRef.current;
    const stage = stageRef.current;
    const wrap = wrapRef.current;
    if (!section || !stage || !wrap) return;

    const update = () => {
      const sectionTop = section.offsetTop;
      const sectionH = section.offsetHeight;
      const stageH = stage.offsetHeight;
      const pinRange = Math.max(1, sectionH - stageH);
      const p = (window.scrollY - sectionTop) / pinRange;
      progressRef.current = Math.max(0, Math.min(1, p));

      const wipe = progressRef.current * 112 - 12;
      const mask = `linear-gradient(to bottom, transparent ${wipe}%, black ${wipe + 12}%)`;
      wrap.style.maskImage = mask;
      wrap.style.webkitMaskImage = mask;
    };

    window.addEventListener("scroll", update, { passive: true });
    window.addEventListener("resize", update);
    update();
    return () => {
      window.removeEventListener("scroll", update);
      window.removeEventListener("resize", update);
    };
  }, []);

  // Mouse parallax — small influence on the diamond rotation.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage) return;
    const onMove = (e: MouseEvent) => {
      const r = stage.getBoundingClientRect();
      mouseRef.current.x = (e.clientX - (r.left + r.width / 2)) / r.width;
      mouseRef.current.y = (e.clientY - (r.top + r.height / 2)) / r.height;
    };
    window.addEventListener("mousemove", onMove);
    return () => window.removeEventListener("mousemove", onMove);
  }, []);

  // ASCII canvas sizing + RAF loop sampling the WebGL canvas.
  useEffect(() => {
    if (reducedMotion) return;
    const ascii = asciiRef.current;
    const stage = stageRef.current;
    if (!ascii || !stage) return;
    const aCtx = ascii.getContext("2d");
    if (!aCtx) return;

    if (!sampleRef.current) {
      sampleRef.current = document.createElement("canvas");
    }
    const sample = sampleRef.current;
    const sCtx = sample.getContext("2d", { willReadFrequently: true });
    if (!sCtx) return;

    let viewW = 0;
    let viewH = 0;
    const startMs = performance.now();
    let raf = 0;
    let cancelled = false;

    const resize = () => {
      const rect = stage.getBoundingClientRect();
      viewW = Math.max(1, Math.floor(rect.width));
      viewH = Math.max(1, Math.floor(rect.height));
      const dpr = Math.min(window.devicePixelRatio, 2);
      ascii.width = viewW * dpr;
      ascii.height = viewH * dpr;
      ascii.style.width = `${viewW}px`;
      ascii.style.height = `${viewH}px`;
      aCtx.setTransform(dpr, 0, 0, dpr, 0, 0);
      aCtx.textBaseline = "top";
    };

    const frame = () => {
      if (cancelled) return;
      const webgl =
        webglRef.current ?? (wrapRef.current?.querySelector("canvas") as HTMLCanvasElement | null);
      if (webgl) {
        webglRef.current = webgl;
        const t = (performance.now() - startMs) / 1000;
        const cols = Math.floor(viewW / CELL_W);
        const rows = Math.floor(viewH / CELL_H);

        aCtx.clearRect(0, 0, viewW, viewH);

        const progress = progressRef.current;
        const fadeStart = 0.78;
        const fadeOut =
          progress < fadeStart ? 1 : Math.max(0, 1 - (progress - fadeStart) / (1 - fadeStart));

        if (cols > 0 && rows > 0 && progress > 0.001 && fadeOut > 0.005) {
          sample.width = cols;
          sample.height = rows;
          try {
            sCtx.drawImage(webgl, 0, 0, cols, rows);
          } catch {
            // canvas not ready yet
            raf = requestAnimationFrame(frame);
            return;
          }
          const img = sCtx.getImageData(0, 0, cols, rows).data;

          aCtx.font = `${CELL_H - 1}px "Berkeley Mono", ui-monospace, monospace`;
          aCtx.textBaseline = "top";

          const bandHalf = Math.max(3, rows * 0.2);
          const front = progress * (rows + bandHalf * 2) - bandHalf;
          const noiseSlow = Math.floor(t * 3);
          const noiseFast = Math.floor(t * 9);

          for (let y = 0; y < rows; y++) {
            const rowJitter = (hashNoise(0, y, noiseSlow) - 0.5) * 1.2;
            for (let x = 0; x < cols; x++) {
              const i = (y * cols + x) * 4;
              const a = img[i + 3];
              if (a < 24) continue;

              const r = img[i];
              const g = img[i + 1];
              const b = img[i + 2];
              const lum = (r * 0.21 + g * 0.72 + b * 0.07) / 255;

              const aboveFront = (front - y) / bandHalf;
              const reveal = Math.max(0, Math.min(1, aboveFront * 0.5 + 0.5));
              if (reveal <= 0) continue;

              const n = hashNoise(x, y, noiseSlow);
              if (n > reveal) continue;
              const twinkle = hashNoise(x + 31, y - 17, noiseFast);

              const edgeBoost = Math.max(0, 1 - Math.abs(aboveFront)) * 0.35;
              const density = Math.min(0.999, lum * 0.85 + 0.06 + edgeBoost);
              const idx = Math.min(RAMP.length - 1, Math.floor(density * RAMP.length));
              const ch = RAMP[idx];

              const onEdge = aboveFront > -0.4 && aboveFront < 0.4;
              const rC = onEdge ? 210 : 140;
              const gC = onEdge ? 230 : 192;
              const bC = 255;

              const base = 0.4 + lum * 0.55 + edgeBoost * 0.6;
              const flicker = 0.78 + twinkle * 0.35;
              const alpha = Math.min(1, base * (0.4 + reveal * 0.85) * flicker * fadeOut);

              aCtx.fillStyle = `rgba(${rC},${gC},${bC},${alpha.toFixed(3)})`;
              aCtx.fillText(ch, x * CELL_W + rowJitter, y * CELL_H);
            }
          }
        }
      }
      raf = requestAnimationFrame(frame);
    };

    resize();
    window.addEventListener("resize", resize);
    raf = requestAnimationFrame(frame);

    return () => {
      cancelled = true;
      cancelAnimationFrame(raf);
      window.removeEventListener("resize", resize);
    };
  }, [reducedMotion]);

  return (
    <section ref={sectionRef} className="diamond-section" aria-hidden="true">
      <div ref={stageRef} className="diamond-stage">
        {reducedMotion ? (
          <div className="ascii-fallback">{DIAMOND_ASCII}</div>
        ) : (
          <>
            <div ref={wrapRef} className="diamond-canvas-wrap">
              <Canvas
                gl={{ antialias: true, alpha: true, preserveDrawingBuffer: true }}
                camera={{ position: [0, 0, 5.6], fov: 38, near: 0.1, far: 100 }}
                dpr={[1, 2]}
              >
                <Diamond mouseRef={mouseRef} />
              </Canvas>
            </div>
            <canvas ref={asciiRef} className="ascii-canvas" />
          </>
        )}
      </div>
    </section>
  );
}
