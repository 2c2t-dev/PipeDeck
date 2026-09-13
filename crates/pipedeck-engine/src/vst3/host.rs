//! Running one VST3 plug-in.
//!
//! The sequence is the one the specification lays out: ask the bundle's
//! factory for the class, initialise it, tell it the sample rate and block
//! size, activate one stereo bus each way, then hand it buffers.
//!
//! Everything here is unsafe by nature: a plug-in is a C++ object reached
//! through a table of function pointers, and it is the plug-in's own code
//! that runs. The rules it imposes are noted where they apply.

use std::ffi::c_void;

use vst3::Steinberg::Vst::{
    AudioBusBuffers, BusDirections_, IAudioProcessor, IAudioProcessorTrait, IComponent,
    IComponentTrait, IParamValueQueue, IParamValueQueueTrait, IParameterChanges,
    IParameterChangesTrait, MediaTypes_, ParamID, ParamValue, ProcessData, ProcessModes_,
    ProcessSetup, SymbolicSampleSizes_,
};
use vst3::Steinberg::{
    int32, kResultFalse, kResultOk, tresult, IPluginBaseTrait, IPluginFactory, IPluginFactoryTrait,
};
use vst3::{Class, ComPtr, ComWrapper, Interface};

use super::scan::{module_factory, Plugin};

/// How many channels a mixer hands a plug-in.
pub const CHANNELS: usize = 2;

/// A queue that accepts points and keeps none.
///
/// A plug-in writes its output parameters into one of these every block. A
/// mixer reads none of them back, but the queue has to exist: plug-ins
/// assert on it rather than check.
struct DiscardQueue;

impl Class for DiscardQueue {
    type Interfaces = (IParamValueQueue,);
}

impl IParamValueQueueTrait for DiscardQueue {
    unsafe fn getParameterId(&self) -> ParamID {
        0
    }

    unsafe fn getPointCount(&self) -> int32 {
        0
    }

    unsafe fn getPoint(
        &self,
        _index: int32,
        _sample_offset: *mut int32,
        _value: *mut ParamValue,
    ) -> tresult {
        kResultFalse
    }

    unsafe fn addPoint(
        &self,
        _sample_offset: int32,
        _value: ParamValue,
        _index: *mut int32,
    ) -> tresult {
        kResultOk
    }
}

/// A list of parameter changes holding none.
///
/// Every block carries one in and one out, and a plug-in is entitled to a
/// real object rather than a null pointer: several assert on it. The mixer
/// changes no parameter mid-block and reads none back, so both are empty.
struct NoParameterChanges {
    /// Handed out by `addParameterData`, which several plug-ins call every
    /// block and none of them expect to fail.
    queue: ComPtr<IParamValueQueue>,
}

impl Class for NoParameterChanges {
    type Interfaces = (IParameterChanges,);
}

impl IParameterChangesTrait for NoParameterChanges {
    unsafe fn getParameterCount(&self) -> int32 {
        0
    }

    unsafe fn getParameterData(&self, _index: int32) -> *mut IParamValueQueue {
        std::ptr::null_mut()
    }

    unsafe fn addParameterData(
        &self,
        _id: *const ParamID,
        index: *mut int32,
    ) -> *mut IParamValueQueue {
        if !index.is_null() {
            *index = 0;
        }
        self.queue.as_ptr()
    }
}

/// A plug-in ready to process audio.
///
/// Dropping it stops and releases the plug-in; the module it came from stays
/// loaded, as hosts keep them for the life of the process.
pub struct Instance {
    // Field order matters: the processor is released before the component
    // that owns it.
    processor: ComPtr<IAudioProcessor>,
    component: ComPtr<IComponent>,
    changes: ComPtr<IParameterChanges>,
    max_block: usize,
    name: String,
}

impl Instance {
    /// Open a plug-in and make it ready for blocks of at most `max_block`
    /// frames at `sample_rate`.
    pub fn open(plugin: &Plugin, sample_rate: f64, max_block: usize) -> Result<Self, String> {
        let factory = module_factory(&plugin.bundle)?;
        let cid = decode_class_id(&plugin.class_id)?;

        // SAFETY: the factory is alive, the ids are 16 bytes as the API
        // demands, and the pointer is only read on success.
        let component: ComPtr<IComponent> = unsafe {
            let mut object: *mut c_void = std::ptr::null_mut();
            let result = factory.createInstance(
                cid.as_ptr(),
                IComponent::IID.as_ptr().cast(),
                &mut object as *mut *mut c_void,
            );
            if result != kResultOk || object.is_null() {
                return Err(format!("{} refused to be created", plugin.name));
            }
            ComPtr::from_raw(object.cast()).ok_or("the component is null")?
        };

        // SAFETY: each call is made once, in the order the specification
        // gives, on an object that is alive for the whole sequence.
        unsafe {
            // No host context: a mixer offers none of the services one
            // carries, and a plug-in that insists will say so here.
            if component.initialize(std::ptr::null_mut()) != kResultOk {
                return Err(format!("{} refused to initialise", plugin.name));
            }

            let processor: ComPtr<IAudioProcessor> = component
                .cast()
                .ok_or_else(|| format!("{} processes no audio", plugin.name))?;

            let mut setup = ProcessSetup {
                processMode: ProcessModes_::kRealtime as i32,
                symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
                maxSamplesPerBlock: max_block as i32,
                sampleRate: sample_rate,
            };
            if processor.setupProcessing(&mut setup) != kResultOk {
                return Err(format!(
                    "{} does not run at {sample_rate} Hz in blocks of {max_block}",
                    plugin.name
                ));
            }

            let audio = MediaTypes_::kAudio as i32;
            component.activateBus(audio, BusDirections_::kInput as i32, 0, 1);
            component.activateBus(audio, BusDirections_::kOutput as i32, 0, 1);
            if component.setActive(1) != kResultOk {
                return Err(format!("{} refused to start", plugin.name));
            }
            processor.setProcessing(1);

            let queue = ComWrapper::new(DiscardQueue)
                .to_com_ptr::<IParamValueQueue>()
                .ok_or("cannot make a parameter queue")?;
            let changes = ComWrapper::new(NoParameterChanges { queue })
                .to_com_ptr::<IParameterChanges>()
                .ok_or("cannot make a parameter list")?;

            Ok(Self {
                processor,
                component,
                changes,
                max_block,
                name: plugin.name.clone(),
            })
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Run one block through the plug-in, in place.
    ///
    /// The buffers are one per channel and must be the same length, no
    /// longer than the block size the plug-in was opened for. A plug-in is
    /// free to write its result straight into the input buffers, which is
    /// why they are the same buffers here.
    pub fn process(&self, channels: &mut [&mut [f32]]) -> Result<(), ()> {
        let frames = channels.first().map_or(0, |channel| channel.len());
        if frames == 0 || frames > self.max_block {
            return Err(());
        }
        if channels.iter().any(|channel| channel.len() != frames) {
            return Err(());
        }

        let mut pointers: Vec<*mut f32> = channels
            .iter_mut()
            .map(|channel| channel.as_mut_ptr())
            .collect();

        let mut bus = AudioBusBuffers {
            numChannels: pointers.len() as i32,
            silenceFlags: 0,
            __field0: vst3::Steinberg::Vst::AudioBusBuffers__type0 {
                channelBuffers32: pointers.as_mut_ptr(),
            },
        };
        let mut output = bus;

        let mut data = ProcessData {
            processMode: ProcessModes_::kRealtime as i32,
            symbolicSampleSize: SymbolicSampleSizes_::kSample32 as i32,
            numSamples: frames as i32,
            numInputs: 1,
            numOutputs: 1,
            inputs: &mut bus,
            outputs: &mut output,
            inputParameterChanges: self.changes.as_ptr(),
            outputParameterChanges: self.changes.as_ptr(),
            inputEvents: std::ptr::null_mut(),
            outputEvents: std::ptr::null_mut(),
            processContext: std::ptr::null_mut(),
        };

        // SAFETY: the buffers outlive the call, every pointer is to one of
        // them, and the counts describe them exactly.
        let result = unsafe { self.processor.process(&mut data) };
        if result == kResultOk {
            Ok(())
        } else {
            Err(())
        }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // SAFETY: undoing the sequence in `open`, on an object still alive
        // because this owns it.
        unsafe {
            self.processor.setProcessing(0);
            self.component.setActive(0);
            self.component.terminate();
        }
    }
}

/// Turn the hex form kept in the config back into the 16 bytes the factory
/// expects.
fn decode_class_id(hex: &str) -> Result<[std::ffi::c_char; 16], String> {
    if hex.len() != 32 {
        return Err(format!("{hex} is not a class id"));
    }
    let mut cid = [0 as std::ffi::c_char; 16];
    for (index, byte) in cid.iter_mut().enumerate() {
        let pair = &hex[index * 2..index * 2 + 2];
        *byte = u8::from_str_radix(pair, 16).map_err(|_| format!("{hex} is not a class id"))?
            as std::ffi::c_char;
    }
    Ok(cid)
}

/// Kept here so the factory type stays private to this module pair.
#[allow(dead_code)]
fn _factory_type(factory: &ComPtr<IPluginFactory>) -> &ComPtr<IPluginFactory> {
    factory
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_class_id_survives_the_round_trip() {
        let raw: Vec<u8> = (0u8..16).collect();
        let hex: String = raw.iter().map(|byte| format!("{byte:02x}")).collect();
        let decoded = decode_class_id(&hex).expect("a class id");
        for (index, byte) in raw.iter().enumerate() {
            assert_eq!(decoded[index] as u8, *byte);
        }
        assert!(decode_class_id("nope").is_err());
    }
}
