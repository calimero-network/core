export interface Theme {
  colors: {
    background: {
      primary: string;
      secondary: string;
      tertiary: string;
    };
    text: {
      primary: string;
      secondary: string;
      error: string;
    };
    accent: {
      primary: string;
      secondary: string;
      disabled: string;
    };
    border: {
      primary: string;
    };
  };
  typography: {
    fontFamily: string;
    title: {
      size: string;
      weight: number;
      lineHeight: string;
    };
    subtitle: {
      size: string;
      weight: number;
      lineHeight: string;
    };
    body: {
      size: string;
      weight: number;
      lineHeight: string;
    };
    small: {
      size: string;
      weight: number;
      lineHeight: string;
    };
  };
  spacing: {
    xs: string;
    sm: string;
    md: string;
    lg: string;
    xl: string;
    xxl: string;
  };
  borderRadius: {
    default: string;
    lg: string;
    sm: string;
  };
  shadows: {
    sm: string;
    default: string;
    lg: string;
  };
  transitions: {
    default: string;
  };
  zIndex: {
    modal: number;
  };
}

const defaultTheme: Theme = {
  colors: {
    background: {
      primary: '#111111',
      secondary: '#1c1c1c',
      tertiary: '#17191b',
    },
    text: {
      primary: '#ffffff',
      secondary: 'rgba(255, 255, 255, 0.7)',
      error: '#ff0000',
    },
    accent: {
      primary: '#ff7a00',
      secondary: '#ff9933',
      disabled: '#666666',
    },
    border: {
      primary: 'rgba(255, 255, 255, 0.1)',
    },
  },
  typography: {
    fontFamily: '-apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, Helvetica, Arial, sans-serif',
    title: {
      size: '1.5rem',
      weight: 700,
      lineHeight: '2rem',
    },
    subtitle: {
      size: '1rem',
      weight: 500,
      lineHeight: '1.25rem',
    },
    body: {
      size: '0.875rem',
      weight: 400,
      lineHeight: '1.5',
    },
    small: {
      size: '0.75rem',
      weight: 400,
      lineHeight: '1.25',
    },
  },
  spacing: {
    xs: '0.25rem',
    sm: '0.5rem',
    md: '1rem',
    lg: '1.5rem',
    xl: '2rem',
    xxl: '3rem',
  },
  borderRadius: {
    default: '0.5rem',
    lg: '0.75rem',
    sm: '0.25rem',
  },
  shadows: {
    sm: '0 1px 2px rgba(0, 0, 0, 0.05)',
    default: '0 1px 3px rgba(0, 0, 0, 0.1), 0 1px 2px rgba(0, 0, 0, 0.06)',
    lg: '0 10px 15px -3px rgba(0, 0, 0, 0.1), 0 4px 6px -2px rgba(0, 0, 0, 0.05)',
  },
  transitions: {
    default: 'all 0.2s ease-in-out',
  },
  zIndex: {
    modal: 1000,
  },
};

export default defaultTheme; 