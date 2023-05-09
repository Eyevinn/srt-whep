from http.server import BaseHTTPRequestHandler, HTTPServer
import threading
import time
import uuid

class MyHandler(BaseHTTPRequestHandler):
    responses = {}

    def do_POST(self):
        content_length = int(self.headers.get('Content-Length', 0))
        content = self.rfile.read(content_length).decode('utf-8')

        # Generate a unique ID for the client
        client_id = str(uuid.uuid4())

        # Store the content in a dictionary with the client ID as the key
        MyHandler.responses[client_id] = content
        print("Receive content:", content)

        # Wait for both clients to submit their requests
        while len(MyHandler.responses) < 2:
            time.sleep(0.1)

        # Determine the status code based on the order of the clients' requests
        if list(MyHandler.responses.keys())[0] == client_id:
            status_code = 200
            # replace "a=setup:actpass" with "a=setup:active"
            response_content = MyHandler.responses[client_id]
            new_response = response_content.replace("a=setup:actpass", "a=setup:active")
            MyHandler.responses[client_id] = new_response
        else:
            status_code = 201
            # replace "a=setup:actpass" with "a=setup:passive"
            response_content = MyHandler.responses[client_id]
            new_response = response_content.replace("a=setup:actpass", "a=setup:passive")
            MyHandler.responses[client_id] = new_response
            
        # Retrieve the content submitted by the other client
        for key, value in MyHandler.responses.items():
            if key != client_id:
                response_content = value

        # Send the other client's content as the response
        self.send_response(status_code)
        self.send_header('Content-type', 'application/sdp')
        self.send_header('Location', 'test')
        self.end_headers()
        self.wfile.write(bytes(response_content, "utf8"))

def serve_forever_in_thread(httpd):
    httpd.serve_forever()

def run(server_class=HTTPServer, handler_class=MyHandler, port=8080):
    server_address = ('', port)
    httpd = server_class(server_address, handler_class)
    print(f"Starting httpd server on port {port}")

    # Start two threads to handle incoming requests
    for _ in range(2):
        t = threading.Thread(target=serve_forever_in_thread, args=(httpd,))
        t.daemon = True
        t.start()

    # Wait for the threads to finish
    t.join()    

if __name__ == '__main__':
    run()