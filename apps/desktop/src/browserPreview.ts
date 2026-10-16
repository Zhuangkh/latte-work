import { invoke } from "@tauri-apps/api/core";

// Temporary, decoded viewport pixels only; release when its overlay closes.
export async function browserPreview(id: string) {
  const bytes = await invoke<ArrayBuffer>("browser_preview", { id });
  if (
    !(bytes instanceof ArrayBuffer) ||
    !bytes.byteLength ||
    bytes.byteLength > 8 * 1024 * 1024
  )
    throw new Error("网页画面暂不可用");
  const url = URL.createObjectURL(new Blob([bytes], { type: "image/jpeg" }));
  let timer: ReturnType<typeof setTimeout> | undefined;
  try {
    const image = new Image();
    image.src = url;
    await Promise.race([
      image.decode(),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error("网页画面读取超时")), 1500);
      }),
    ]);
    return { url, release: () => URL.revokeObjectURL(url) };
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  } finally {
    clearTimeout(timer);
  }
}
