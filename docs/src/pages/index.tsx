import clsx from "clsx";
import Link from "@docusaurus/Link";
import useDocusaurusContext from "@docusaurus/useDocusaurusContext";
import Layout from "@theme/Layout";
import HomepageFeatures from "@site/src/components/HomepageFeatures";
import Heading from "@theme/Heading";

import styles from "./index.module.css";

function HomepageHeader() {
  const { siteConfig } = useDocusaurusContext();
  return (
    <header className={clsx("", styles.heroBanner)}>
      <div className="container">
        <Heading as="h1" className="hero__title">
          {siteConfig.title}
        </Heading>
      </div>
    </header>
  );
}

function HomepageBody() {
  return (
    <header className={styles.features}>
      <div className={styles.bodyContainer}>
        <div className={styles.textPadding}>
          You're about to dive into the Calimero Network, a place designed to
          shake up the digital world by prioritizing what matters most: privacy,
          data control, and freedom in your creations. Calimero offers a
          foundation for those committed to building the new digital landscape
          where privacy and user autonomy are non-negotiable. It's a shift
          towards an ecosystem where applications are built on principles of
          decentralization, ensuring users retain control over their digital
          footprint.
        </div>
      </div>
      <div className={styles.buttons}>
        <Link
          className="button button--secondary button--lg"
          to="/core/explore/intro"
        >
          Dive into privacy preserving technology
        </Link>
      </div>
    </header>
  );
}

export default function Home(): JSX.Element {
  const { siteConfig } = useDocusaurusContext();
  return (
    <Layout
      title={`Hello from ${siteConfig.title}`}
      description="Description will go into a meta tag in <head />"
    >
      <HomepageHeader />
      <main>
        <HomepageBody />
        <HomepageFeatures />
      </main>
    </Layout>
  );
}
