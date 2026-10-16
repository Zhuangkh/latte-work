import { useState, useSyncExternalStore } from "react";
import { Link2 } from "lucide-react";
import { webLink } from "./browserLinks";

const icons = new Map<string, string>();
const listeners = new Set<() => void>();
export function rememberSiteIcon(url: string, icon?: string) {
  const address = webLink(url);
  if (
    !address ||
    !icon ||
    icon.length > 96 * 1024 ||
    !/^data:image\/[a-z.+-]+;base64,[A-Za-z0-9+/=]+$/.test(icon)
  )
    return;
  if (icons.get(address) === icon) return;
  icons.delete(address);
  icons.set(address, icon);
  if (icons.size > 64) icons.delete(icons.keys().next().value!);
  listeners.forEach((listener) => listener());
}
function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
export function SiteIcon({
  url,
  className = "",
}: {
  url: string;
  className?: string;
}) {
  const address = webLink(url);
  const cached = useSyncExternalStore(subscribe, () =>
    address ? icons.get(address) : undefined,
  );
  const source =
    cached || (address ? `${new URL(address).origin}/favicon.ico` : "");
  const [loaded, setLoaded] = useState("");
  const [failed, setFailed] = useState("");
  const ready = !!source && loaded === source;
  return (
    <span className={`site-icon ${className}`} aria-hidden="true">
      {!ready && <Link2 size={16} />}
      {source && failed !== source && (
        <img
          src={source}
          alt=""
          width={16}
          height={16}
          loading="lazy"
          decoding="async"
          referrerPolicy="no-referrer"
          className={ready ? "is-ready" : undefined}
          onLoad={() => setLoaded(source)}
          onError={() => setFailed(source)}
        />
      )}
    </span>
  );
}
