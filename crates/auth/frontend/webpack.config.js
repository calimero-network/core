const path = require('path');
const webpack = require('webpack');

module.exports = {
  entry: './index.js',
  output: {
    path: path.resolve(__dirname, 'dist'),
    filename: 'bundle.js',
  },
  devServer: {
    static: {
      directory: path.join(__dirname, './'),
    },
    compress: true,
    port: 9000,
    hot: true,
  },
  resolve: {
    fallback: {
      'buffer': require.resolve('buffer/'),
      'crypto': require.resolve('crypto-browserify'),
      'stream': require.resolve('stream-browserify'),
      'util': require.resolve('util/'),
      'process': require.resolve('process/browser'),
      'http': false,
      'https': false,
      'zlib': require.resolve('browserify-zlib'),
    },
    alias: {
      'near-api-js/lib/providers': path.resolve(__dirname, 'node_modules/near-api-js/lib/providers/index.js'),
      'near-api-js/lib/utils': path.resolve(__dirname, 'node_modules/near-api-js/lib/utils/index.js'),
      'near-api-js/lib/transaction': path.resolve(__dirname, 'node_modules/near-api-js/lib/transaction.js')
    }
  },
  plugins: [
    // Work around for Buffer is undefined:
    // https://github.com/webpack/changelog-v5/issues/10
    new webpack.ProvidePlugin({
      Buffer: ['buffer', 'Buffer'],
      process: 'process/browser',
    }),
    // Add a global Buffer object
    new webpack.DefinePlugin({
      'global.Buffer': 'Buffer',
    }),
  ]
}; 