import { ChangesView } from "./ChangesView";
import type { Project } from "./protocol";
/** Historical entry points select a scope in the same change page. */
export function TaskChangesView(props: {
  hostId: string;
  project: Project;
  sessionId?: string;
  requestId?: string;
  initialPath?: string;
  active: boolean;
}) {
  return (
    <ChangesView
      key={JSON.stringify([
        props.hostId,
        props.project.id,
        props.sessionId,
        props.requestId,
      ])}
      {...props}
      initialScope={
        props.requestId ? "turn" : props.sessionId ? "task" : "branch"
      }
    />
  );
}
