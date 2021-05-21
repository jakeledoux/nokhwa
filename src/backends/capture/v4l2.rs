use crate::{
    error::NokhwaError,
    utils::{CameraFormat, CameraInfo},
    CaptureBackendTrait, FrameFormat, Resolution,
};
use v4l::prelude::*;
use v4l::{
    buffer::Type,
    io::traits::CaptureStream,
    video::{capture::Parameters, Capture},
    Format, FourCC,
};

#[cfg(feature = "input_v4l")]
impl From<CameraFormat> for Format {
    fn from(cam_fmt: CameraFormat) -> Self {
        let pxfmt = match cam_fmt.format() {
            FrameFormat::MJPEG => FourCC::new(b"MJPG"),
            FrameFormat::YUYV => FourCC::new(b"YUYV"),
        };

        Format::new(cam_fmt.width(), cam_fmt.height(), pxfmt)
    }
}

/// The backend struct that interfaces with V4L2.
/// To see what this does, please see [`CaptureBackendTrait`]
/// # Quirks
/// Calling [`set_resolution()`](CaptureBackendTrait::set_resolution), [`set_framerate()`](CaptureBackendTrait::set_framerate), or [`set_frameformat()`](CaptureBackendTrait::set_frameformat)
/// each internally calls [`set_camera_format()`](CaptureBackendTrait::set_camera_format).
pub struct V4LCaptureDevice<'a> {
    camera_format: Option<CameraFormat>,
    camera_info: CameraInfo,
    device: Device,
    stream_handle: Option<MmapStream<'a>>,
}

impl<'a> V4LCaptureDevice<'a> {
    /// Creates a new capture device using the V4L2 backend. Indexes are gives to devices by the OS, and usually numbered by order of discovery.
    /// # Errors
    /// This function will error if the camera is currently busy or if V4L2 can't read device information.
    pub fn new(index: usize) -> Result<Self, NokhwaError> {
        let device = match Device::new(index) {
            Ok(dev) => dev,
            Err(why) => {
                return Err(NokhwaError::CouldntOpenDevice(format!(
                    "V4L2 Error: {}",
                    why.to_string()
                )))
            }
        };

        let camera_info = match device.query_caps() {
            Ok(caps) => CameraInfo::new(caps.card, "".to_string(), caps.driver, index),
            Err(why) => {
                return Err(NokhwaError::CouldntQueryDevice {
                    property: "Capabilities".to_string(),
                    error: why.to_string(),
                })
            }
        };

        Ok(V4LCaptureDevice {
            camera_format: None,
            camera_info,
            device,
            stream_handle: None,
        })
    }
}

impl<'a> CaptureBackendTrait for V4LCaptureDevice<'a> {
    fn get_info(&self) -> CameraInfo {
        self.camera_info.clone()
    }

    fn get_camera_format(&self) -> Option<CameraFormat> {
        self.camera_format
    }

    #[allow(clippy::option_if_let_else)]
    fn init_camera_format_default(&mut self, overwrite: bool) -> Result<(), NokhwaError> {
        match self.camera_format {
            Some(_) => {
                if overwrite {
                    return self.set_camera_format(CameraFormat::default());
                }
                Ok(())
            }
            None => self.set_camera_format(CameraFormat::default()),
        }
    }

    fn set_camera_format(&mut self, new_fmt: CameraFormat) -> Result<(), NokhwaError> {
        let prev_format = match self.device.format() {
            Ok(fmt) => fmt,
            Err(why) => {
                return Err(NokhwaError::CouldntQueryDevice {
                    property: "Resolution, FrameFormat".to_string(),
                    error: why.to_string(),
                })
            }
        };
        let prev_fps = match self.device.params() {
            Ok(fps) => fps,
            Err(why) => {
                return Err(NokhwaError::CouldntQueryDevice {
                    property: "Framerate".to_string(),
                    error: why.to_string(),
                })
            }
        };

        let format: Format = new_fmt.into();
        let framerate = Parameters::with_fps(new_fmt.framerate());

        if let Err(why) = self.device.set_format(&format) {
            return Err(NokhwaError::CouldntSetProperty {
                property: "Resolution, FrameFormat".to_string(),
                value: format.to_string(),
                error: why.to_string(),
            });
        }
        if let Err(why) = self.device.set_params(&framerate) {
            return Err(NokhwaError::CouldntSetProperty {
                property: "Framerate".to_string(),
                value: framerate.to_string(),
                error: why.to_string(),
            });
        }

        if self.stream_handle.is_some() {
            self.stream_handle = Some({
                match MmapStream::new(&self.device, Type::VideoCapture) {
                    Ok(stream) => stream,
                    Err(why) => {
                        // undo
                        if let Err(why) = self.device.set_format(&prev_format) {
                            return Err(NokhwaError::CouldntSetProperty {
                                property: "Attempt undo due to stream acquisition failure. Resolution, FrameFormat".to_string(),
                                value: prev_format.to_string(),
                                error: why.to_string(),
                            });
                        }
                        if let Err(why) = self.device.set_params(&prev_fps) {
                            return Err(NokhwaError::CouldntSetProperty {
                                property:
                                    "Attempt undo due to stream acquisition failure. Framerate"
                                        .to_string(),
                                value: prev_fps.to_string(),
                                error: why.to_string(),
                            });
                        }

                        return Err(NokhwaError::CouldntOpenStream(why.to_string()));
                    }
                }
            })
        }
        self.camera_format = Some(new_fmt);
        Ok(())
    }

    fn get_resolution(&self) -> Option<Resolution> {
        self.camera_format.map(|fmt| fmt.resoltuion())
    }

    #[allow(clippy::option_if_let_else)]
    fn set_resolution(&mut self, new_res: Resolution) -> Result<(), NokhwaError> {
        if let Some(fmt) = self.camera_format {
            let mut new_fmt = fmt;
            new_fmt.set_resolution(new_res);
            self.set_camera_format(new_fmt)
        } else {
            self.camera_format = Some(CameraFormat::new(new_res, FrameFormat::MJPEG, 0));
            Ok(())
        }
    }

    fn get_framerate(&self) -> Option<u32> {
        self.camera_format.map(|fmt| fmt.framerate())
    }

    #[allow(clippy::option_if_let_else)]
    fn set_framerate(&mut self, new_fps: u32) -> Result<(), NokhwaError> {
        if let Some(fmt) = self.camera_format {
            let mut new_fmt = fmt;
            new_fmt.set_framerate(new_fps);
            self.set_camera_format(new_fmt)
        } else {
            self.camera_format = Some(CameraFormat::new(
                Resolution::new(0, 0),
                FrameFormat::MJPEG,
                new_fps,
            ));
            Ok(())
        }
    }

    fn get_frameformat(&self) -> Option<FrameFormat> {
        self.camera_format.map(|fmt| fmt.format())
    }

    #[allow(clippy::option_if_let_else)]
    fn set_frameformat(&mut self, fourcc: FrameFormat) -> Result<(), NokhwaError> {
        if let Some(fmt) = self.camera_format {
            let mut new_fmt = fmt;
            new_fmt.set_format(fourcc);
            self.set_camera_format(new_fmt)
        } else {
            self.camera_format = Some(CameraFormat::new(Resolution::new(0, 0), fourcc, 0));
            Ok(())
        }
    }

    fn open_stream(&mut self) -> Result<(), NokhwaError> {
        todo!()
    }

    fn is_stream_open(&self) -> bool {
        self.stream_handle.is_some()
    }

    fn get_frame(&self) -> Result<image::ImageBuffer<image::Rgb<u8>, Vec<u8>>, NokhwaError> {
        todo!()
    }

    fn get_frame_raw(&self) -> Vec<u8> {
        todo!()
    }
}
