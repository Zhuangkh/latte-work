import { WebLink } from "./LinkHandling";
import { webLink } from "./browserLinks";
import { SiteIcon } from "./SiteIcon";

export function TaskLinkRow({ url, title }: { url: string; title: string }) {
  const address = webLink(url);
  const parsed = address ? new URL(address) : null;
  const compact = parsed
    ? `${parsed.host}${parsed.pathname === "/" ? "" : parsed.pathname}${parsed.search}${parsed.hash}`
    : url;
  const label = title.trim();
  const hasTitle =
    !!label &&
    ![url, address, compact, parsed?.hostname, parsed?.host].includes(label);
  const display = hasTitle ? label : compact;
  return (
    <WebLink
      href={url}
      className="task-info-file task-info-web"
      aria-label={display}
    >
      <SiteIcon
        key={parsed?.origin || url}
        url={url}
        className="task-info-site-icon"
      />
      <span>
        <span className="task-info-resource-title" title={display}>
          {display}
        </span>
        {hasTitle && <small>{compact}</small>}
      </span>
    </WebLink>
  );
}
