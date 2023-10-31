Planned stuff:

1. Service registration: Allow users to register their background services with your tool. This could involve providing a name, description, and configuration details for each service.

2. Start and stop services: Implement functionality to start and stop registered services. Users should be able to initiate the execution of a service or terminate its execution.

3. Service status monitoring: Provide a way for users to check the status of their registered services. This could include displaying whether a service is running, stopped, or encountering any issues.

4. Basic logging: Implement logging functionality to capture important events or errors related to the execution of background services. This can help users troubleshoot issues and monitor the behavior of their services.

5. Health checks: Allow users to define health checks for their services. This could involve running periodic checks to ensure that a service is functioning correctly and reporting any failures or anomalies.

6. Configuration management: Provide a way for users to configure and update the settings for their registered services. This could include options to modify environment variables, command-line arguments, or other relevant configurations.

7. Basic command-line interface (CLI): Create a simple command-line interface that allows users to interact with your tool and perform actions like registering services, starting/stopping services, and checking service status.

8. Error handling and user feedback: Implement proper error handling to provide meaningful error messages and feedback to users when they encounter issues or invalid inputs.

WILL NOT WORK ON MACOS BECAUSE OF A BUG from `notify` crate, with `FSEvent API` 
