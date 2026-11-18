import * as React from "react";

export type IconProps = React.SVGProps<SVGSVGElement> & {
  size?: number | string;
  strokeWidth?: number | string;
  absoluteStrokeWidth?: boolean;
};

const Shield = React.forwardRef<SVGSVGElement, IconProps>(
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
        viewBox="0 0 38 34"
        role="presentation"
        aria-hidden={true}
        fill="none"
        stroke="currentColor"
        strokeWidth={actualStrokeWidth}
        strokeLinecap="round"
        strokeLinejoin="round"
        {...props}
      >
        <path d="M34 18.1434L19 32.9994L4.00001 18.1434C3.01062 17.1806 2.23129 16.0234 1.7111 14.7446C1.19091 13.4659 0.94113 12.0933 0.977482 10.7132C1.01383 9.33321 1.33553 7.97565 1.92232 6.72605C2.50911 5.47645 3.34828 4.36189 4.38698 3.45254C5.42568 2.54319 6.64142 1.85875 7.95763 1.44234C9.27384 1.02592 10.662 0.886545 12.0347 1.03298C13.4075 1.17941 14.735 1.60849 15.9338 2.29319C17.1325 2.97789 18.1765 3.90338 19 5.01138C19.8271 3.91142 20.8723 2.99402 22.0702 2.31659C23.2681 1.63915 24.593 1.21628 25.9619 1.07442C27.3307 0.932558 28.7142 1.07477 30.0256 1.49216C31.3369 1.90955 32.5481 2.59312 33.5831 3.5001C34.6182 4.40708 35.4549 5.51795 36.0408 6.76316C36.6268 8.00838 36.9495 9.36114 36.9886 10.7368C37.0278 12.1124 36.7825 13.4814 36.2683 14.7579C35.754 16.0344 34.9818 17.191 34 18.1554" />
      </svg>
    );
  },
);

Shield.displayName = "Shield";
export { Shield };
