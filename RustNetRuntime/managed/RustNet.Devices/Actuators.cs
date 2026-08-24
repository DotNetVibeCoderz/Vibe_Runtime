using RustNet.Hal;

namespace RustNet.Devices;

/// <summary>Single LED on a GPIO pin.</summary>
public class Led
{
    private readonly int _pin;

    public Led(int pin)
    {
        _pin = pin;
        Gpio.SetMode(pin, PinMode.Output);
    }

    public void On() => Gpio.Write(_pin, true);
    public void Off() => Gpio.Write(_pin, false);
    public void Toggle() => Gpio.Toggle(_pin);
}

/// <summary>Relay module (active-high or active-low).</summary>
public class Relay
{
    private readonly int _pin;
    private readonly bool _activeLow;

    public Relay(int pin) : this(pin, false)
    {
    }

    public Relay(int pin, bool activeLow)
    {
        _pin = pin;
        _activeLow = activeLow;
        Gpio.SetMode(pin, PinMode.Output);
        Open();
    }

    /// <summary>Energize the relay (close the switched circuit).</summary>
    public void Close() => Gpio.Write(_pin, !_activeLow);

    /// <summary>De-energize the relay (open the switched circuit).</summary>
    public void Open() => Gpio.Write(_pin, _activeLow);
}

/// <summary>Push button with pull-up wiring (pressed = low).</summary>
public class Button
{
    private readonly int _pin;

    public Button(int pin)
    {
        _pin = pin;
        Gpio.SetMode(pin, PinMode.InputPullUp);
    }

    public bool IsPressed() => !Gpio.Read(_pin);
}

/// <summary>DC motor on an H-bridge: PWM speed + direction pin.</summary>
public class MotorDriver
{
    private readonly int _pwmChannel;
    private readonly int _dirPin;
    private readonly int _frequencyHz;

    public MotorDriver(int pwmChannel, int dirPin)
    {
        _pwmChannel = pwmChannel;
        _dirPin = dirPin;
        _frequencyHz = 20000;
        Gpio.SetMode(dirPin, PinMode.Output);
    }

    /// <summary>speedPercent: -100 (full reverse) .. 100 (full forward).</summary>
    public void SetSpeed(int speedPercent)
    {
        bool forward = speedPercent >= 0;
        int magnitude = speedPercent;
        if (magnitude < 0)
        {
            magnitude = -magnitude;
        }
        if (magnitude > 100)
        {
            magnitude = 100;
        }
        Gpio.Write(_dirPin, forward);
        Pwm.Configure(_pwmChannel, _frequencyHz, magnitude * 100);
    }

    public void Stop() => Pwm.Configure(_pwmChannel, _frequencyHz, 0);
}

/// <summary>Servo on a PWM channel (50 Hz, 1-2 ms pulse).</summary>
public class Servo
{
    private readonly int _channel;

    public Servo(int pwmChannel)
    {
        _channel = pwmChannel;
    }

    /// <summary>angle: 0..180 degrees.</summary>
    public void SetAngle(int angle)
    {
        if (angle < 0)
        {
            angle = 0;
        }
        if (angle > 180)
        {
            angle = 180;
        }
        // 50 Hz period = 20 ms. 0 deg = 1 ms (5%), 180 deg = 2 ms (10%).
        int duty = 500 + angle * 500 / 180;
        Pwm.Configure(_channel, 50, duty);
    }
}
