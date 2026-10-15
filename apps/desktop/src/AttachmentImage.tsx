import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { Image, LoaderCircle, X } from "lucide-react";
import { message } from "./api";
import { loadSource } from "./sourceLoader";
import { Modal } from "./Modal";
import type { ComposerReference } from "./pasteAttachments";

// A conversation can contain many images. Queue reads only for visible images,
// and share a two-reader bound across composer, transcript and enlarged previews.
const queue: (() => Promise<void>)[] = [];
let readers = 0;
function pump() {
  while (readers < 2 && queue.length) {
    readers++;
    void queue.shift()!().finally(() => {
      readers--;
      pump();
    });
  }
}
function imageRead<T>(
  read: () => Promise<T>,
  valid: () => boolean,
): Promise<T> {
  return new Promise((resolve, reject) => {
    if (queue.length >= 64) {
      reject(new Error("图片预览繁忙，请稍后重试"));
      return;
    }
    queue.push(async () => {
      try {
        if (!valid()) throw new Error("图片预览已取消");
        resolve(await read());
      } catch (error) {
        reject(error);
      }
    });
    pump();
  });
}

export function AttachmentImage({
  reference,
  hostId,
  projectId,
  sessionId,
  connected,
  active = true,
  variant,
  onOpen,
}: {
  reference: ComposerReference;
  hostId: string;
  projectId?: string;
  sessionId?: string;
  connected: boolean;
  active?: boolean;
  variant: "chip" | "thumbnail";
  onOpen?: () => void;
}) {
  const target = useRef<HTMLButtonElement>(null);
  const [visible, setVisible] = useState(false);
  const [open, setOpen] = useState(false);
  const [revision, retry] = useState(0);
  const scope = JSON.stringify([
    hostId,
    projectId,
    sessionId,
    reference.path,
    revision,
  ]);
  const identity = JSON.stringify([
    hostId,
    projectId,
    sessionId,
    reference.path,
    active,
  ]);
  useLayoutEffect(() => {
    setOpen(false);
  }, [identity]);
  useEffect(() => {
    if (!active || variant !== "thumbnail" || !target.current) return;
    if (typeof IntersectionObserver === "undefined") {
      setVisible(true);
      return;
    }
    let disposed = false;
    const observer = new IntersectionObserver(
      (entries) => {
        if (!disposed)
          setVisible(entries.some((entry) => entry.isIntersecting));
      },
      { rootMargin: "120px" },
    );
    observer.observe(target.current);
    return () => {
      disposed = true;
      observer.disconnect();
    };
  }, [identity, variant, active]);
  const enabled = active && (open || (variant === "thumbnail" && visible));
  const [state, setState] = useState<{
    scope: string;
    url?: string;
    error?: string;
    loading?: boolean;
  }>({ scope });
  const resource = useRef<{ scope: string; url: string } | undefined>(
    undefined,
  );
  function release() {
    if (resource.current) URL.revokeObjectURL(resource.current.url);
    resource.current = undefined;
  }
  useEffect(() => {
    if (!enabled) {
      setState({ scope });
      return;
    }
    return release;
  }, [scope, enabled]);
  useEffect(() => {
    if (!enabled || resource.current?.scope === scope) return;
    if (!connected || !projectId) {
      setState({
        scope,
        error: !connected ? "连接主机后可预览图片" : "缺少图片所在的项目",
      });
      return;
    }
    let disposed = false;
    const valid = () => !disposed;
    setState({ scope, loading: true });
    void imageRead(
      () => loadSource(hostId, projectId, sessionId, reference.path, valid),
      valid,
    )
      .then((data) => {
        if (disposed) return;
        if (
          !/^(image\/(png|jpeg|gif|webp|bmp|tiff|avif|heic|heif))$/.test(
            data.mime,
          ) ||
          data.truncated
        )
          throw new Error("此文件不能作为图片预览");
        const url = URL.createObjectURL(
          new Blob([data.bytes as BlobPart], { type: data.mime }),
        );
        resource.current = { scope, url };
        setState({ scope, url });
      })
      .catch((error) => {
        if (!disposed) setState({ scope, error: message(error) });
      });
    return () => {
      disposed = true;
    };
  }, [scope, enabled, connected, projectId, hostId, sessionId, reference.path]);
  const preview = state.scope === scope && enabled ? state : { scope };
  function failedImage() {
    release();
    setState({ scope, error: "图片格式无法解码，请重试" });
  }
  return (
    <>
      <button
        className={
          variant === "chip"
            ? "attachment-image-chip"
            : "attachment-image-thumbnail"
        }
        ref={target}
        type="button"
        title={reference.path}
        aria-label={`预览图片 ${reference.name}`}
        onClick={() => {
          onOpen?.();
          target.current?.focus();
          setOpen(true);
        }}
      >
        {variant === "thumbnail" && preview.url ? (
          <img src={preview.url} alt={reference.name} onError={failedImage} />
        ) : (
          <Image size={variant === "chip" ? 14 : 22} aria-hidden="true" />
        )}
        {variant === "chip" ? (
          <span>图片 · {reference.name}</span>
        ) : (
          !preview.url && (
            <span>{preview.loading ? "加载图片…" : reference.name}</span>
          )
        )}
      </button>
      {open &&
        active &&
        createPortal(
          <div
            className="modal-backdrop attachment-image-backdrop"
            onClick={(event) => {
              if (event.target === event.currentTarget) setOpen(false);
            }}
          >
            <Modal
              className="modal attachment-image-dialog"
              aria-label={`图片预览 ${reference.name}`}
              onKeyDown={(event) => {
                if (event.key === "Escape" && !event.defaultPrevented) {
                  event.preventDefault();
                  event.stopPropagation();
                  setOpen(false);
                }
              }}
            >
              <header>
                <span title={reference.path}>{reference.name}</span>
                <button
                  className="icon-button"
                  aria-label="关闭图片预览"
                  onClick={() => setOpen(false)}
                >
                  <X size={18} />
                </button>
              </header>
              <div className="attachment-image-stage">
                {preview.url ? (
                  <img
                    src={preview.url}
                    alt={reference.name}
                    onError={failedImage}
                  />
                ) : preview.error ? (
                  <div className="attachment-image-error" role="alert">
                    <Image size={28} />
                    <p>{preview.error}</p>
                    <button
                      className="secondary small"
                      disabled={!connected || !projectId}
                      onClick={() => retry((value) => value + 1)}
                    >
                      重试
                    </button>
                  </div>
                ) : (
                  <div className="attachment-image-loading" role="status">
                    <LoaderCircle size={22} className="spin" />
                    正在读取图片…
                  </div>
                )}
              </div>
            </Modal>
          </div>,
          document.body,
        )}
    </>
  );
}
