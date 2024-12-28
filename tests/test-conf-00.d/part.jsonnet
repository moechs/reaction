local banFor(time) = {
  ban: {
    cmd: ['ban', '<mypat>'],
  },
  unban: {
    after: time,
    cmd: ['unban', '<mypat>'],
  }
};
{
  streams: {
    common: {
      filters: {
        from_jsonnet: {
          regex: [@'^<mypat>'],
          actions: banFor('42s'),
        }
      }
    }
  }
}
