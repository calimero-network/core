import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Zap = React.forwardRef<SVGSVGElement, IconProps>(
  (
    { size = 24, strokeWidth = 1.5, absoluteStrokeWidth = false, ...props },
    ref,
  ) => {
    const actualStrokeWidth = absoluteStrokeWidth
      ? strokeWidth
      : (Number(size) / 24) * Number(strokeWidth);

    return (
      <svg
        ref={ref}
        width={size}
        height={size}
        viewBox="0 0 48 48"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M24.8 6C26.8899 6.0013 28.9335 6.61576 30.6777 7.76724C32.4218 8.91872 33.7897 10.5566 34.612 12.478C36.004 12.6349 37.3515 13.0645 38.5775 13.7422C39.8035 14.4199 40.884 15.3324 41.7573 16.4277C42.6306 17.5229 43.2797 18.7795 43.6674 20.1256C44.0551 21.4717 44.1739 22.881 44.017 24.273C43.8601 25.665 43.4305 27.0125 42.7528 28.2385C42.0751 29.4645 41.1626 30.545 40.0673 31.4183C38.9721 32.2917 37.7155 32.9407 36.3694 33.3284C35.0233 33.7161 33.614 33.8349 32.222 33.678C31.2089 35.47 29.5813 36.8346 27.6401 37.5195C25.6989 38.2045 23.5753 38.1634 21.662 37.404L14 42V35.732C13.2173 35.551 12.4855 35.1957 11.8593 34.6924C11.2332 34.1891 10.7287 33.551 10.3835 32.8255C10.0384 32.1001 9.86154 31.3061 9.86612 30.5028C9.8707 29.6994 10.0566 28.9075 10.41 28.186C8.50049 27.0875 7.02688 25.3657 6.2364 23.3095C5.44591 21.2533 5.38665 18.9878 6.06855 16.8931C6.75046 14.7983 8.13204 13.0019 9.9815 11.805C11.831 10.6082 14.0358 10.0839 16.226 10.32C17.2177 8.97953 18.51 7.89039 19.9991 7.14011C21.4882 6.38983 23.1326 5.99933 24.8 6Z" />
      </svg>
    );
  },
);

Zap.displayName = "Zap";
export { Zap };
