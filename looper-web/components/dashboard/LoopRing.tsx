function pointOnCircle(cx: number, cy: number, radius: number, angleDegrees: number) {
  const angle = ((angleDegrees - 90) * Math.PI) / 180;
  return {
    x: cx + radius * Math.cos(angle),
    y: cy + radius * Math.sin(angle),
  };
}

function arcPath(radius: number, startAngle: number, endAngle: number) {
  const center = 160;
  const start = pointOnCircle(center, center, radius, startAngle);
  const end = pointOnCircle(center, center, radius, endAngle);
  const largeArc = endAngle - startAngle <= 180 ? 0 : 1;
  return `M ${start.x} ${start.y} A ${radius} ${radius} 0 ${largeArc} 1 ${end.x} ${end.y}`;
}

type LoopRingProps = {
  title: string;
  modelLabel: string;
  steps: string[];
  activeStep: number | null;
  totalLoops: number;
  rotationDegrees?: number;
};

export function LoopRing({
  title,
  modelLabel,
  steps,
  activeStep,
  totalLoops,
  rotationDegrees = 0,
}: LoopRingProps) {
  const ringRadius = 122;
  const labelRadius = 145;
  const segmentGap = 8;

  return (
    <div className="p-2">
      <h3 className="mb-3 text-center text-base font-semibold">{title}</h3>
      <p className="mb-3 text-center text-xs text-zinc-600 dark:text-zinc-300">Total loops: {totalLoops}</p>
      <div className="relative mx-auto h-80 w-80">
        <svg viewBox="0 0 320 320" className="h-full w-full">
          {steps.map((step, index) => {
            const baseStart = index * 120 + rotationDegrees;
            const start = baseStart + segmentGap;
            const end = baseStart + 120 - segmentGap;
            const isActive = activeStep === index;

            return (
              <path
                key={step}
                d={arcPath(ringRadius, start, end)}
                fill="none"
                strokeLinecap="round"
                className={
                  isActive
                    ? "stroke-[14] text-black transition-colors duration-500 dark:text-white"
                    : "stroke-[14] text-zinc-300 transition-colors duration-500 dark:text-zinc-700"
                }
                stroke="currentColor"
              />
            );
          })}
        </svg>

        <div className="absolute inset-0 flex items-center justify-center">
          <div className="flex h-44 w-44 items-center justify-center rounded-full border border-zinc-300 bg-zinc-50 px-6 text-center text-sm font-medium dark:border-zinc-700 dark:bg-zinc-900">
            {modelLabel}
          </div>
        </div>

        {steps.map((step, index) => {
          const angle = index * 120 + 60 + rotationDegrees;
          const point = pointOnCircle(160, 160, labelRadius, angle);
          const isActive = activeStep === index;

          return (
            <div
              key={`${step}-label`}
              className={`absolute w-32 -translate-x-1/2 -translate-y-1/2 rounded-xl border px-2 py-1 text-center text-xs leading-tight transition-colors duration-500 ${
                isActive
                  ? "border-zinc-300 bg-zinc-100 text-zinc-900 dark:border-zinc-600 dark:bg-zinc-800 dark:text-zinc-50"
                  : "border-zinc-200 bg-white text-zinc-600 dark:border-zinc-700 dark:bg-zinc-900 dark:text-zinc-300"
              }`}
              style={{ left: point.x, top: point.y }}
            >
              {step}
            </div>
          );
        })}
      </div>
    </div>
  );
}
