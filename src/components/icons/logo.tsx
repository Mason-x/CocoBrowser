/**
 * CocoBrowser mark: a coconut shell with its three germination pores.
 *
 * Drawn as a single even-odd path so it renders correctly as a monochrome
 * template icon (macOS menu bar, system tray) at any size — the pores stay
 * legible down to 16px, and `currentColor` lets it follow the theme.
 */
export const Logo = (props: React.SVGProps<SVGSVGElement>) => (
  <svg
    xmlns="http://www.w3.org/2000/svg"
    width={1200}
    height={1200}
    role="graphics-symbol img"
    fill="currentColor"
    viewBox="0 0 900 900"
    {...props}
  >
    <title>CocoBrowser</title>
    <path
      fillRule="evenodd"
      clipRule="evenodd"
      d="M60 450a390 390 0 1 0 780 0 390 390 0 1 0-780 0Zm243-75a52 52 0 1 0 104 0 52 52 0 1 0-104 0Zm190 0a52 52 0 1 0 104 0 52 52 0 1 0-104 0Zm-101 195a58 58 0 1 0 116 0 58 58 0 1 0-116 0Z"
    />
  </svg>
);
