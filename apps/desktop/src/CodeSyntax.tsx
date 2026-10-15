const keywords = new Set(
  "as async await break case catch class const continue def delete do else enum export extern false finally fn for from function if impl import in interface let loop match mod move mut new None null of pass private pub public return self static struct super switch this throw trait true try type typeof use var void while with yield".split(
    " ",
  ),
);
/** Render escaped lexical tokens; unfamiliar file types remain plain text. */
export function CodeSyntax({ text, path }: { text: string; path?: string }) {
  if (
    !path ||
    !/\.(rs|[cm]?[jt]sx?|py|json|css|sh|bash|zsh|ya?ml|toml)$/.test(path)
  )
    return <>{text}</>;
  const hashComments = /\.(py|sh|bash|zsh|ya?ml|toml)$/.test(path);
  const tokens = text.split(
    /(\/\/[^\n]*|#[^\n]*|\/\*[\s\S]*?\*\/|"(?:\\.|[^"\\])*"|'(?:\\.|[^'\\])*'|`(?:\\.|[^`\\])*`|\b\d+(?:\.\d+)?\b|\b[A-Za-z_][A-Za-z_0-9]*\b)/g,
  );
  return (
    <>
      {tokens.map((token, index) => {
        const kind =
          /^(\/\/|\/\*)/.test(token) || (hashComments && token.startsWith("#"))
            ? "comment"
            : /^["'`]/.test(token)
              ? "string"
              : /^\d/.test(token)
                ? "number"
                : keywords.has(token)
                  ? "keyword"
                  : /^[A-Z]\w*$/.test(token)
                    ? "type"
                    : "";
        return kind ? (
          <span key={index} className={`syntax-${kind}`}>
            {token}
          </span>
        ) : (
          token
        );
      })}
    </>
  );
}
