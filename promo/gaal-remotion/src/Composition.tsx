import {
  AbsoluteFill,
  Easing,
  interpolate,
  Sequence,
  useCurrentFrame,
} from "remotion";

const mono =
  '"SFMono-Regular", "SF Mono", "JetBrains Mono", "Fira Code", ui-monospace, Menlo, Consolas, monospace';

const fade = (frame: number, start: number, end: number) =>
  interpolate(frame, [start, end], [0, 1], {
    easing: Easing.bezier(0.16, 1, 0.3, 1),
    extrapolateLeft: "clamp",
    extrapolateRight: "clamp",
  });

const TerminalWindow: React.FC<{
  title: string;
  children: React.ReactNode;
  width?: number;
}> = ({ title, children, width = 820 }) => {
  return (
    <div
      style={{
        width,
        borderRadius: 14,
        border: "1px solid rgba(148, 163, 184, 0.24)",
        background: "rgba(9, 13, 21, 0.92)",
        boxShadow: "0 28px 90px rgba(0, 0, 0, 0.34)",
        overflow: "hidden",
        fontFamily: mono,
      }}
    >
      <div
        style={{
          height: 40,
          display: "flex",
          alignItems: "center",
          gap: 9,
          padding: "0 18px",
          color: "#94a3b8",
          background: "rgba(15, 23, 42, 0.92)",
          fontSize: 16,
        }}
      >
        <span style={{ width: 11, height: 11, borderRadius: 99, background: "#fb7185" }} />
        <span style={{ width: 11, height: 11, borderRadius: 99, background: "#fbbf24" }} />
        <span style={{ width: 11, height: 11, borderRadius: 99, background: "#34d399" }} />
        <span style={{ marginLeft: 12 }}>{title}</span>
      </div>
      <div style={{ padding: "28px 32px 30px" }}>{children}</div>
    </div>
  );
};

const Line: React.FC<{
  text: string;
  delay: number;
  accent?: boolean;
  muted?: boolean;
}> = ({ text, delay, accent = false, muted = false }) => {
  const frame = useCurrentFrame();
  const opacity = fade(frame, delay, delay + 14);

  return (
    <div
      style={{
        opacity,
        translate: `0 ${interpolate(opacity, [0, 1], [10, 0])}px`,
        minHeight: 33,
        color: accent ? "#67e8f9" : muted ? "#64748b" : "#dbeafe",
        fontSize: 24,
        lineHeight: "33px",
        letterSpacing: 0,
        whiteSpace: "pre-wrap",
      }}
    >
      {text}
    </div>
  );
};

const Badge: React.FC<{ label: string; delay: number }> = ({ label, delay }) => {
  const frame = useCurrentFrame();
  const opacity = fade(frame, delay, delay + 16);

  return (
    <div
      style={{
        opacity,
        translate: `0 ${interpolate(opacity, [0, 1], [14, 0])}px`,
        border: "1px solid rgba(103, 232, 249, 0.28)",
        background: "rgba(8, 145, 178, 0.12)",
        color: "#a5f3fc",
        borderRadius: 8,
        padding: "12px 15px",
        fontSize: 20,
        fontFamily: mono,
      }}
    >
      {label}
    </div>
  );
};

const Grid = () => (
  <AbsoluteFill
    style={{
      opacity: 0.24,
      backgroundImage:
        "linear-gradient(rgba(148,163,184,0.16) 1px, transparent 1px), linear-gradient(90deg, rgba(148,163,184,0.16) 1px, transparent 1px)",
      backgroundSize: "48px 48px",
    }}
  />
);

const Intro = () => {
  const frame = useCurrentFrame();

  return (
    <AbsoluteFill
      style={{
        justifyContent: "center",
        paddingLeft: 94,
        opacity: fade(frame, 0, 20) - fade(frame, 190, 220),
      }}
    >
      <div style={{ color: "#94a3b8", fontSize: 25, fontFamily: mono, marginBottom: 22 }}>
        local agent work has a memory problem
      </div>
      <div
        style={{
          color: "#f8fafc",
          fontSize: 74,
          lineHeight: "82px",
          fontWeight: 760,
          maxWidth: 960,
          letterSpacing: 0,
        }}
      >
        Your agents did the work.
        <br />
        The evidence is buried.
      </div>
    </AbsoluteFill>
  );
};

const IndexScene = () => (
  <AbsoluteFill style={{ alignItems: "center", justifyContent: "center" }}>
    <TerminalWindow title="~/projects/gaal">
      <Line text="$ gaal index backfill" delay={0} accent />
      <Line text="scanning local traces..." delay={28} />
      <Line text="codex       1,284 sessions" delay={54} />
      <Line text="claude        739 sessions" delay={76} />
      <Line text="gemini        118 sessions" delay={98} />
      <Line text="agy            82 sessions" delay={120} />
      <Line text="ok: normalized into queryable artifacts" delay={150} accent />
    </TerminalWindow>
  </AbsoluteFill>
);

const QueryScene = () => (
  <AbsoluteFill style={{ alignItems: "center", justifyContent: "center" }}>
    <TerminalWindow title="ask practical questions" width={920}>
      <Line text="$ gaal who wrote src/main.rs --since 30d -H" delay={0} accent />
      <Line text="session 4a17843a  fixed handoff attribution" delay={32} />
      <Line text="session bd914364  repaired transcript rendering" delay={54} />
      <Line text={'$ gaal search "migration error" --field all'} delay={94} accent />
      <Line text="3 matching sessions, transcripts ready" delay={126} />
      <Line text={'$ gaal recall "release prep" --limit 3 -H'} delay={164} accent />
      <Line text="handoffs found. context restored." delay={196} />
    </TerminalWindow>
  </AbsoluteFill>
);

const OutcomeScene = () => {
  const frame = useCurrentFrame();

  return (
    <AbsoluteFill style={{ justifyContent: "center", padding: "0 92px" }}>
      <div
        style={{
          opacity: fade(frame, 0, 28),
          color: "#f8fafc",
          fontSize: 86,
          fontWeight: 780,
          letterSpacing: 0,
          lineHeight: "92px",
        }}
      >
        Gaal
      </div>
      <div
        style={{
          opacity: fade(frame, 20, 48),
          color: "#cbd5e1",
          fontSize: 38,
          marginTop: 20,
          lineHeight: "50px",
          maxWidth: 980,
        }}
      >
        Local memory for AI coding-agent work.
        <br />
        Not a cloud. Not a daemon. Just evidence.
      </div>
      <div style={{ display: "flex", gap: 16, marginTop: 48 }}>
        <Badge label="searchable traces" delay={64} />
        <Badge label="attribution" delay={82} />
        <Badge label="handoff-ready" delay={100} />
      </div>
    </AbsoluteFill>
  );
};

export const MyComposition = () => {
  return (
    <AbsoluteFill
      style={{
        background:
          "radial-gradient(circle at 70% 20%, rgba(34, 211, 238, 0.16), transparent 32%), #070a10",
        color: "#f8fafc",
        fontFamily:
          'Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif',
      }}
    >
      <Grid />
      <Sequence durationInFrames={230} premountFor={30}>
        <Intro />
      </Sequence>
      <Sequence from={220} durationInFrames={250} premountFor={30}>
        <IndexScene />
      </Sequence>
      <Sequence from={465} durationInFrames={285} premountFor={30}>
        <QueryScene />
      </Sequence>
      <Sequence from={745} durationInFrames={155} premountFor={30}>
        <OutcomeScene />
      </Sequence>
    </AbsoluteFill>
  );
};
