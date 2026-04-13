import * as React from 'react';

type NativeImgProps = Omit<React.ImgHTMLAttributes<HTMLImageElement>, 'src'>;

interface NextImageLikeProps extends NativeImgProps {
  src: string;
  alt: string;
  width?: number;
  height?: number;
  fill?: boolean;
}

const Image = React.forwardRef<HTMLImageElement, NextImageLikeProps>(function Image(
  { src, alt, width, height, fill, style, ...rest },
  ref,
) {
  return (
    <img
      ref={ref}
      src={src}
      alt={alt}
      width={fill ? undefined : width}
      height={fill ? undefined : height}
      style={fill ? { ...style, width: '100%', height: '100%', objectFit: 'cover' } : style}
      {...rest}
    />
  );
});

export default Image;
