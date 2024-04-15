import clsx from 'clsx';
import Heading from '@theme/Heading';
import styles from './styles.module.css';

type FeatureItem = {
  title: string;
  Svg: React.ComponentType<React.ComponentProps<'svg'>>;
  description: JSX.Element;
};


const FeatureList: FeatureItem[] = [
  {
    title: 'Redefining Privacy',
    Svg: require('@site/static/img/undraw_docusaurus_mountain.svg').default,
    description: (
      <>
        Our Core Crusade
Privacy is the bedrock of our digital freedoms. In a realm rife with surveillance, we draw a line. Our mission, powered by zero-knowledge, multiparty computation, homomorphic encryption, and beyond, is to enshrine privacy as a fundamental right, transforming it from a mere concept into an everyday reality for all.
      </>
    ),
  },
  {
    title: 'Championing Data Sovereignty',
    Svg: require('@site/static/img/undraw_docusaurus_tree.svg').default,
    description: (
      <>
        The era of corporations ruling our digital lives is ending. With Calimero, you ascend to sovereignty over your data. We're dismantling the old structures, ensuring that your data—your digital essence—is yours to control, share, and monetize. Envision a digital realm where your data empowers you, not corporate giants.
      </>
    ),
  },
  {
    title: 'Defending the Digital Dialogue',
    Svg: require('@site/static/img/undraw_docusaurus_react.svg').default,
    description: (
      <>
        In the battle against censorship's dark veil, cast by both states and corporations, Calimero emerges as a fortress of free speech. We're committed to protecting the diversity of voices, ensuring that every perspective is heard and valued. Our pledge is unwavering: to safeguard the flow of ideas, keeping the digital realm a vibrant forum for debate and discovery.      </>
    ),
  },
];

function Feature({title, Svg, description}: FeatureItem) {
  return (
    <div className={clsx('col col--4')}>
      <div className="text--center">
        <Svg className={styles.featureSvg} role="img" />
      </div>
      <div className="text--center padding-horiz--md">
        <Heading as="h3">{title}</Heading>
        <p>{description}</p>
      </div>
    </div>
  );
}

export default function HomepageFeatures(): JSX.Element {
  return (
    <section className={styles.features}>
      <div className="container">
        <div className="row">
          {FeatureList.map((props, idx) => (
            <Feature key={idx} {...props} />
          ))}
        </div>
      </div>
    </section>
  );
}
