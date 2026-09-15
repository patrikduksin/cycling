# Read-only request candidates

These are profile-level diagnostic payloads, not a profile API. No transmission
or device response was observed while preparing this note.

## Heart rate

`46 ff ff ff ff 01 06 01` requests one broadcast of heart-rate page 6,
capabilities. The expected page number is `data[0] & 0x7f == 6`; byte 1 is
reserved, bytes 2 and 3 describe supported and enabled features. This request
reads capabilities and does not enable them. The ANT Heart Rate Profile describes
page 6 as sent on request. Capture a baseline first and look for its appearance
after submission. H10 support remains unverified. A missing response does not
establish failed radio delivery.

Heart-rate monitors use profile-specific background pages, not common pages 80
or 81. The request's response field is `01`, one broadcast response. The profile
requires displays to request broadcast responses even though the request itself
uses an acknowledged message. Source: [ANT Heart Rate Profile Rev 2.1,
sections 6.6.7 and 6.7.1](https://homepages.laas.fr/rcayre/assets/ressources/ant_hrm_device_profile.pdf).

## Radar

`46 ff ff ff ff 08 50 01` requests eight broadcasts of manufacturer page 80.
Bytes 1 through 4 are reserved or invalid descriptors, byte 5 is the bounded
response count, byte 6 is the requested page and byte 7 selects data-page request.
The radar profile requires replies to requests for pages 80, 81 and 82 and
requires displays to request broadcast responses. Source: [ANT Bike Radar Profile
Rev 2.1, section 6.11.1, table 6-14](https://devzone.nordicsemi.com/cfs-file/__key/support-attachments/beef5d1b77644c448dabff31668f3a47-40ef7acdda7a482aa67b000fa33e8a3f/D00001665_5F002D005F00_ANT_2B005F00_Device_5F002D005F00_Bike_5F00_Radar_5F00_2.1.pdf).

Page 80 also appears periodically. An isolated post-request page cannot establish
causation. Compare its baseline count and spacing with the requested group of
eight responses. This is an experimental inference, not a transaction identifier
or generic delivery acknowledgment. SR mini compliance and actual response
scheduling remain to be measured.
