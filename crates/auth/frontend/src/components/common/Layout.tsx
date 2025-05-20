import React from 'react';
import styled from '@emotion/styled';

const Container = styled.div`
  max-width: 600px;
  margin: 0 auto;
  padding: 40px 20px;
  min-height: 100vh;
  display: flex;
  flex-direction: column;
`;

const Header = styled.header`
  text-align: center;
  margin-bottom: 40px;
`;

const Title = styled.h1`
  font-size: 24px;
  font-weight: 600;
  color: #333;
  margin: 0;
`;

const Content = styled.main`
  flex: 1;
`;

const Footer = styled.footer`
  margin-top: 40px;
  text-align: center;
  font-size: 14px;
  color: #777;
`;

interface LayoutProps {
  children: React.ReactNode;
}

const Layout: React.FC<LayoutProps> = ({ children }) => {
  return (
    <Container>
      <Header>
        <Title>Calimero Authentication</Title>
      </Header>
      
      <Content>
        {children}
      </Content>
      
      <Footer>
        &copy; {new Date().getFullYear()} Calimero Network
      </Footer>
    </Container>
  );
};

export default Layout;