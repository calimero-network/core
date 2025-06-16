import Button from './Button';
import { ErrorContainer, ErrorMessage } from './styles';

interface ErrorViewProps {
  message: string;
  onRetry?: () => void;
  buttonText?: string;
}

export function ErrorView({ message, onRetry, buttonText }: ErrorViewProps) {
  const handleRefresh = () => {
    if (onRetry) {
      onRetry();
    } else {
      window.location.reload();
    }
  };

  return (
    <ErrorContainer>
      <ErrorMessage>{message}</ErrorMessage>
      <Button onClick={handleRefresh} size="md">
        {buttonText || 'Try Again'}
      </Button>
    </ErrorContainer>
  );
} 