import { RemoteGlobeMark } from "./RemoteGlobeIcon";

export function RemoteProjectIcon({
  state = "connected",
  label = "远程项目",
  title = label,
}: {
  state?: "connecting" | "connected" | "failed";
  label?: string;
  title?: string;
}) {
  return (
    <svg
      width="16"
      height="16"
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.75"
      strokeLinecap="round"
      strokeLinejoin="round"
      role="img"
      aria-label={label}
      data-state={state}
    >
      <title>{title}</title>
      <path d="M9 20H4a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h3.93a2 2 0 0 1 1.67.9l.81 1.2A2 2 0 0 0 12.1 6H20a2 2 0 0 1 2 2v3M2 10h20" />
      <g
        className={
          state === "failed" ? "project-remote-error" : "project-remote-globe"
        }
        strokeWidth="1.5"
      >
        {state === "failed" ? (
          <>
            <circle cx="18" cy="18" r="4.5" />
            <path d="M18 15.5v3M18 20.5h.01" />
          </>
        ) : (
          <RemoteGlobeMark />
        )}
      </g>
    </svg>
  );
}
