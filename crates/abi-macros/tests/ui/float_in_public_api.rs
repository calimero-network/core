use calimero_sdk::app;

#[app::state]
struct MyApp {
    value: f32, //~ ERROR: floats are not supported; use fixed-point or string
}

#[app::logic]
impl MyApp {
    #[app::init]
    pub fn init() -> Self {
        Self { value: 0.0 }
    }
    
    pub fn set_value(&mut self, value: f64) -> app::Result<()> {
        //~ ERROR: floats are not supported; use fixed-point or string
        self.value = value as f32;
        Ok(())
    }
    
    pub fn get_value(&self) -> f32 {
        //~ ERROR: floats are not supported; use fixed-point or string
        self.value
    }
} 